#!/usr/bin/env python3
"""Validate and package WSL2 ext4 GNU/musl Phase 0 store evidence."""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess


ARCHITECTURE_COMMIT = "65e60216891bb3d826a4778f84cb8aaa377abe92"
ARCHITECTURE_MANIFEST = (
    "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0"
)
EXPECTED_BACKENDS = {"redb", "sqlite"}
EXPECTED_SOURCE_COUNT = 19
EXPECTED_FAULT_CASES = {
    "backend-contract|indexed-query",
    "backend-contract|corruption-visible",
    "publication|before-attempt-catalog-allocation",
    "publication|after-catalog-allocation",
    "publication|after-running-envelope",
    "publication|after-latest-running",
    "publication|after-run-rename",
    "publication|after-terminal-attempt",
    "publication|after-latest-temp",
    "publication|after-latest-replace",
    "publication-concurrency|reverse-sequence-independent-fields",
    "publication-concurrency|same-sequence-terminal-beats-running",
    "publication-retention-race|publication-first-makes-retention-stale",
    "publication-retention-race|retention-first-blocks-publication",
    "retention|before-prepared-plan",
    "retention|after-prepared-plan",
    "retention|after-pruning-commit",
    "retention|after-payload-move",
    "retention|after-pruned-commit",
    "retention|after-physical-reclamation",
    "retention-integrity|both-canonical-and-trash",
    "retention-integrity|neither-canonical-nor-trash",
    "migration|before-migration-intent",
    "migration|after-migration-intent",
    "migration|after-validated-replacement",
    "migration|after-canonical-replace",
    "migration|after-intent-removal",
    "migration|stale-generation-writer",
    "namespace|state-directory-copy-swap",
    "namespace|lifecycle-lock-replacement",
    "namespace|lifecycle-lock-content-mutation",
    "namespace|lifecycle-lock-extra-link",
    "namespace|attempts-parent-replacement",
    "namespace|runs-parent-replacement",
    "namespace|trash-parent-replacement",
    "namespace|cache-parent-replacement",
    "namespace|attempts-anchor-replacement",
    "namespace|runs-anchor-replacement",
    "namespace|trash-anchor-replacement",
    "namespace|cache-anchor-replacement",
    "namespace|runs-anchor-content-mutation",
    "namespace|runs-anchor-extra-link",
    "namespace|runs-parent-replacement-after-run-rename",
    "namespace|runs-parent-replacement-before-final-commit",
    "namespace|trash-parent-replacement-before-trash-move",
    "namespace|trash-parent-replacement-after-trash-move",
    "namespace|trash-parent-replacement-before-final-commit",
}
MODES = {
    "gnu": {
        "target": "x86_64-unknown-linux-gnu",
        "admission": "admission-linux-gnu-ext4-x64.json",
        "fault": "fault-matrix-linux-gnu-ext4-x64.json",
        "build": "build-surface-linux-gnu-x64.json",
        "harness_identity": "identity-gnu.json",
        "linkage": "linkage-gnu.txt",
    },
    "musl": {
        "target": "x86_64-unknown-linux-musl",
        "admission": "admission-linux-musl-ext4-x64.json",
        "fault": "fault-matrix-linux-musl-ext4-x64.json",
        "build": "build-surface-linux-musl-x64.json",
        "harness_identity": "identity-musl.json",
        "linkage": "linkage-musl.txt",
    },
}
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_json(path: Path) -> dict[str, object]:
    if not path.is_file():
        raise RuntimeError(f"missing evidence file: {path}")
    return json.loads(path.read_text(encoding="utf-8"))


def assert_candidate(document: dict[str, object], name: str) -> None:
    if document.get("architecture_commit") != ARCHITECTURE_COMMIT:
        raise RuntimeError(f"{name} architecture commit mismatch")
    if document.get("architecture_manifest_sha256") != ARCHITECTURE_MANIFEST:
        raise RuntimeError(f"{name} architecture manifest mismatch")


def assert_backend_set(items: list[dict[str, object]], name: str) -> None:
    names = [item.get("backend") for item in items]
    if len(names) != len(EXPECTED_BACKENDS) or set(names) != EXPECTED_BACKENDS:
        raise RuntimeError(f"{name} backend set mismatch: {names}")


def source_fingerprint(document: dict[str, object], name: str) -> str:
    sources = document.get("source_files")
    if not isinstance(sources, list) or len(sources) != EXPECTED_SOURCE_COUNT:
        raise RuntimeError(f"{name} must carry exactly {EXPECTED_SOURCE_COUNT} sources")
    seen: set[str] = set()
    lines: list[str] = []
    for source in sources:
        path = source.get("path")
        digest = source.get("sha256")
        if not isinstance(path, str) or not path or not isinstance(digest, str):
            raise RuntimeError(f"{name} has malformed source identity")
        if not HEX_SHA256.fullmatch(digest) or path in seen:
            raise RuntimeError(f"{name} has invalid or repeated source identity {path}")
        seen.add(path)
        lines.append(f"{digest}  {path}\n")
    fingerprint = "".join(sorted(lines))
    digest = hashlib.sha256(fingerprint.encode("utf-8")).hexdigest()
    if digest != document.get("source_manifest_sha256"):
        raise RuntimeError(f"{name} source manifest digest mismatch")
    return fingerprint


def assert_current_source(document: dict[str, object], source_root: Path) -> None:
    for item in document["source_files"]:
        path = (source_root / item["path"]).resolve(strict=True)
        path.relative_to(source_root)
        if sha256(path) != item["sha256"]:
            raise RuntimeError(f"current source differs from executable: {item['path']}")


def assert_executable_identity(
    document: dict[str, object], expected_sha: str, expected_bytes: int, name: str
) -> None:
    if (
        document.get("executable_sha256") != expected_sha
        or document.get("executable_bytes") != expected_bytes
        or not HEX_SHA256.fullmatch(expected_sha)
        or expected_bytes <= 0
    ):
        raise RuntimeError(f"{name} executable identity mismatch")


def resolve_recorded_artifact(source: Path, evidence: Path, recorded: str) -> Path:
    source_path = (source / recorded).resolve()
    if source_path.is_file():
        return source_path
    packet_path = (evidence / Path(recorded).name).resolve(strict=True)
    packet_path.relative_to(evidence)
    return packet_path


def live_identity(binary: Path) -> dict[str, object]:
    completed = subprocess.run(
        [str(binary), "identity"],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return json.loads(completed.stdout)


def assert_admission(document: dict[str, object], name: str) -> None:
    if document.get("overall_status") != "PASS" or document.get("rounds") != 32:
        raise RuntimeError(f"{name} did not pass exactly 32 rounds")
    if document.get("host_os") != "linux" or document.get("host_arch") != "x86_64":
        raise RuntimeError(f"{name} host identity mismatch")
    backends = document["backends"]
    assert_backend_set(backends, name)
    for backend in backends:
        if backend.get("status") != "PASS" or backend.get("error") is not None:
            raise RuntimeError(f"{name}/{backend['backend']} failed")
        contention = backend.get("contention_rounds")
        disjoint = backend.get("disjoint_rounds")
        if len(contention) != 32 or len(disjoint) != 32:
            raise RuntimeError(f"{name}/{backend['backend']} round count mismatch")
        for round_result in contention:
            outcomes = Counter(
                child["outcome"]["status"] for child in round_result["child_results"]
            )
            if not round_result["conflicting"] or outcomes != {"admitted": 1, "conflict": 1}:
                raise RuntimeError(f"{name} contention truth mismatch")
        for round_result in disjoint:
            outcomes = Counter(
                child["outcome"]["status"] for child in round_result["child_results"]
            )
            if round_result["conflicting"] or outcomes != {"admitted": 2}:
                raise RuntimeError(f"{name} disjoint truth mismatch")
        phases = {case["phase"] for case in backend.get("crash_cases", [])}
        if phases != {"uncommitted", "committed"}:
            raise RuntimeError(f"{name}/{backend['backend']} crash cases mismatch")


def assert_fault_matrix(document: dict[str, object], name: str) -> int:
    if document.get("overall_status") != "PASS":
        raise RuntimeError(f"{name} did not pass")
    if document.get("host_os") != "linux" or document.get("host_arch") != "x86_64":
        raise RuntimeError(f"{name} host identity mismatch")
    backends = document["backends"]
    assert_backend_set(backends, name)
    namespace_count = 0
    for backend in backends:
        cases = backend["cases"]
        keys = [f"{case['domain']}|{case['crash_point']}" for case in cases]
        if (
            backend.get("status") != "PASS"
            or len(keys) != len(EXPECTED_FAULT_CASES)
            or len(set(keys)) != len(keys)
            or set(keys) != EXPECTED_FAULT_CASES
        ):
            raise RuntimeError(f"{name}/{backend['backend']} case identity mismatch")
        for case in cases:
            if case.get("status") != "PASS" or case.get("error") is not None:
                raise RuntimeError(f"{name}/{backend['backend']} case failed")
            if case["domain"] != "namespace":
                continue
            observation = case["observation"]
            child = observation.get("child_result")
            if (
                observation.get("injection_outcome") != "injected-and-detected"
                or not isinstance(child, dict)
                or not child.get("hard_stop")
                or child.get("canonical_commit_written")
                or observation.get("canonical_commit_written")
            ):
                raise RuntimeError(f"{name}/{backend['backend']} namespace truth mismatch")
            namespace_count += 1
    return namespace_count


def assert_linkage(path: Path, mode: str) -> None:
    text = path.read_text(encoding="utf-8")
    if mode == "musl":
        if "statically linked" not in text or "not a dynamic executable" not in text:
            raise RuntimeError(f"{path.name} is not proven static")
        if "INTERP" in text or "NEEDED" in text:
            raise RuntimeError(f"{path.name} contains a dynamic linkage marker")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--offline", action="store_true")
    arguments = parser.parse_args()
    source = arguments.source.resolve(strict=True)
    evidence = arguments.evidence.resolve(strict=True)
    collector_path = Path(__file__).with_name("collect-build-metrics.py").resolve(strict=True)
    verifier_path = Path(__file__).resolve(strict=True)
    source_reference: str | None = None
    namespace_total = 0
    measurements: list[dict[str, object]] = []
    build_reports: dict[str, dict[str, object]] = {}
    raw_files: set[str] = set()

    for mode, spec in MODES.items():
        documents = {
            "admission": load_json(evidence / spec["admission"]),
            "fault": load_json(evidence / spec["fault"]),
            "build": load_json(evidence / spec["build"]),
        }
        raw_files.update((spec["admission"], spec["fault"], spec["build"]))
        for label, document in documents.items():
            assert_candidate(document, f"{mode}/{label}")
        admission = documents["admission"]
        fault = documents["fault"]
        build = documents["build"]
        assert_admission(admission, f"{mode}/admission")
        namespace_total += assert_fault_matrix(fault, f"{mode}/fault")
        if build.get("mode") != mode or build.get("target") != spec["target"]:
            raise RuntimeError(f"{mode} build target mismatch")
        if "ext4" not in build.get("filesystem", "") or "microsoft" not in build.get(
            "host_uname", ""
        ).lower():
            raise RuntimeError(f"{mode} build is not bound to WSL2 ext4")
        if build.get("collector_sha256") != sha256(collector_path):
            raise RuntimeError(f"{mode} collector identity mismatch")
        assert_backend_set(build["results"], f"{mode}/build")
        build_reports[mode] = build

        admission_source = source_fingerprint(admission, f"{mode}/admission")
        if source_reference is None:
            source_reference = admission_source
            assert_current_source(admission, source)
        elif admission_source != source_reference:
            raise RuntimeError(f"{mode} source identity differs from prior mode")
        for label, document in (("fault", fault),):
            if source_fingerprint(document, f"{mode}/{label}") != source_reference:
                raise RuntimeError(f"{mode}/{label} source identity mismatch")

        harness = build["harness_executable"]
        assert_executable_identity(
            admission, harness["sha256"], harness["bytes"], f"{mode}/admission"
        )
        assert_executable_identity(fault, harness["sha256"], harness["bytes"], f"{mode}/fault")
        recorded_harness_identity = load_json(evidence / spec["harness_identity"])
        raw_files.add(spec["harness_identity"])
        assert_candidate(recorded_harness_identity, f"{mode}/harness identity")
        assert_executable_identity(
            recorded_harness_identity,
            harness["sha256"],
            harness["bytes"],
            f"{mode}/harness identity",
        )
        if source_fingerprint(recorded_harness_identity, f"{mode}/harness identity") != source_reference:
            raise RuntimeError(f"{mode} harness source identity mismatch")
        if not arguments.offline:
            harness_path = Path(harness["path"]).resolve(strict=True)
            if sha256(harness_path) != harness["sha256"] or harness_path.stat().st_size != harness["bytes"]:
                raise RuntimeError(f"{mode} live harness file identity mismatch")
            live = live_identity(harness_path)
            if live != recorded_harness_identity:
                raise RuntimeError(f"{mode} recorded harness identity differs from live output")

        linkage_path = evidence / spec["linkage"]
        assert_linkage(linkage_path, mode)
        raw_files.add(spec["linkage"])
        by_backend = {result["backend"]: result for result in build["results"]}
        for backend in sorted(EXPECTED_BACKENDS):
            result = by_backend[backend]
            if (
                not HEX_SHA256.fullmatch(result.get("binary_sha256", ""))
                or result.get("binary_bytes", 0) <= 0
            ):
                raise RuntimeError(f"{mode}/{backend} build identity invalid")
            for phase in ("clean_build", "incremental_build"):
                record = result[phase]
                artifact = resolve_recorded_artifact(source, evidence, record["log"])
                if sha256(artifact) != record["log_sha256"]:
                    raise RuntimeError(f"{mode}/{backend} {phase} log mismatch")
                raw_files.add(artifact.name)
            tree_record = result["dependency_surface"]
            tree = resolve_recorded_artifact(source, evidence, tree_record["dependency_tree"])
            if sha256(tree) != tree_record["dependency_tree_sha256"]:
                raise RuntimeError(f"{mode}/{backend} dependency tree mismatch")
            raw_files.add(tree.name)

            benchmark_name = f"benchmark-{backend}-linux-{mode}-x64.json"
            benchmark = load_json(evidence / benchmark_name)
            raw_files.add(benchmark_name)
            assert_candidate(benchmark, f"{mode}/{backend} benchmark")
            if benchmark.get("status") != "PASS" or benchmark.get("backend") != backend:
                raise RuntimeError(f"{mode}/{backend} benchmark failed")
            assert_executable_identity(
                benchmark,
                result["binary_sha256"],
                result["binary_bytes"],
                f"{mode}/{backend} benchmark",
            )
            if source_fingerprint(benchmark, f"{mode}/{backend} benchmark") != source_reference:
                raise RuntimeError(f"{mode}/{backend} benchmark source mismatch")

            identity_name = f"identity-{mode}-{backend}.json"
            recorded_identity = load_json(evidence / identity_name)
            raw_files.add(identity_name)
            assert_candidate(recorded_identity, f"{mode}/{backend} identity")
            assert_executable_identity(
                recorded_identity,
                result["binary_sha256"],
                result["binary_bytes"],
                f"{mode}/{backend} identity",
            )
            if source_fingerprint(recorded_identity, f"{mode}/{backend} identity") != source_reference:
                raise RuntimeError(f"{mode}/{backend} identity source mismatch")
            if not arguments.offline:
                binary = (source / result["binary"]).resolve(strict=True)
                if sha256(binary) != result["binary_sha256"] or binary.stat().st_size != result["binary_bytes"]:
                    raise RuntimeError(f"{mode}/{backend} live binary identity mismatch")
                if live_identity(binary) != recorded_identity:
                    raise RuntimeError(f"{mode}/{backend} recorded identity differs from live output")
            if mode == "musl":
                linkage_name = f"linkage-musl-{backend}.txt"
                assert_linkage(evidence / linkage_name, mode)
                raw_files.add(linkage_name)

            measurements.append(
                {
                    "mode": mode,
                    "backend": backend,
                    "executable_sha256": benchmark["executable_sha256"],
                    "initialize_micros": benchmark["initialize_micros"],
                    "bulk_insert_micros": benchmark["bulk_insert_micros"],
                    "first_reopen_query_micros": benchmark["first_reopen_query_micros"],
                    "warm_reopen_query": benchmark["warm_reopen_query"],
                    "durable_admission": benchmark["durable_admission"],
                    "peak_working_set_bytes": benchmark["peak_working_set_bytes"],
                    "store_bytes": benchmark["store_bytes"],
                    "binary_bytes": result["binary_bytes"],
                    "clean_build_millis": result["clean_build"]["elapsed_millis"],
                    "incremental_build_millis": result["incremental_build"]["elapsed_millis"],
                    "transitive_package_count": tree_record["transitive_package_count"],
                    "rust_unsafe_keyword_line_count": tree_record[
                        "rust_unsafe_keyword_line_count"
                    ],
                    "native_source_file_count": tree_record["native_source_file_count"],
                    "native_source_bytes": tree_record["native_source_bytes"],
                }
            )

    raw_files.update(
        {
            "build-gnu-all.log",
            "build-gnu-all-seconds.txt",
            "build-musl-all.log",
            "build-musl-all-seconds.txt",
        }
    )
    if source_reference is None:
        raise RuntimeError("no source identity was validated")
    source_manifest_sha256 = hashlib.sha256(source_reference.encode("utf-8")).hexdigest()
    summary = {
        "probe_id": "lumin-store-wsl2-ext4-musl-x64-evidence-summary-v1",
        "scope": "wsl2-ext4-gnu-musl-partial-phase0-evidence",
        "status": "PASS",
        "architecture_commit": ARCHITECTURE_COMMIT,
        "architecture_manifest_sha256": ARCHITECTURE_MANIFEST,
        "source_manifest_sha256": source_manifest_sha256,
        "backend_selected": False,
        "toolchain": {
            "rustc": build_reports["gnu"]["rustc"],
            "cargo": build_reports["gnu"]["cargo"],
            "zig": build_reports["musl"]["zig"],
            "zig_distribution_sha256": (
                "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00"
            ),
            "cargo_zigbuild": build_reports["musl"]["cargo_zigbuild"],
        },
        "collector_sha256": sha256(collector_path),
        "verifier_sha256": sha256(verifier_path),
        "correctness": {
            "modes": sorted(MODES),
            "admission_rounds_per_backend_per_mode": 32,
            "fault_cases_per_backend_per_mode": len(EXPECTED_FAULT_CASES),
            "total_fault_cases": len(EXPECTED_FAULT_CASES)
            * len(EXPECTED_BACKENDS)
            * len(MODES),
            "namespace_injected_and_detected": namespace_total,
            "namespace_kernel_prevented_before_displacement": 0,
        },
        "harnesses": {
            mode: build_reports[mode]["harness_executable"] for mode in sorted(MODES)
        },
        "measurements": measurements,
        "interpretation": [
            "Correctness passed for the exact GNU and musl case sets on this WSL2 ext4 host.",
            "All namespace replacements were injected and detected after displacement.",
            "The musl harness and feature binaries are static ELF executables without dynamic NEEDED entries.",
            "The first reopen query is not an operating-system cold-cache measurement.",
            "The unsafe keyword count is a comparison surface, not a safety audit.",
            "No backend is selected until all blocking native platform and correctness evidence passes.",
        ],
        "pending": [
            "native non-WSL Linux ext4 and musl package/filesystem evidence",
            "remaining required filesystem durable-flush and lock semantics",
            "native path/root and packaged skill evidence",
            "OXC memory and worker-stack evidence",
            "approved cross-platform numeric budgets",
        ],
    }
    summary_name = "wsl2-ext4-musl-x64-summary.json"
    summary_path = evidence / summary_name
    summary_path.write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    raw_files.add(summary_name)
    manifest_lines = []
    for name in sorted(raw_files):
        path = evidence / name
        if not path.is_file():
            raise RuntimeError(f"missing manifest input: {path}")
        manifest_lines.append(f"{sha256(path)}  {name}\n")
    (evidence / "SHA256SUMS").write_text(
        "".join(manifest_lines), encoding="utf-8", newline="\n"
    )
    print(summary_path)
    print(evidence / "SHA256SUMS")


if __name__ == "__main__":
    main()
