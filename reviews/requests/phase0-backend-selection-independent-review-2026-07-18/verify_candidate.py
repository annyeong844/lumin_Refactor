#!/usr/bin/env python3
"""Verify the Phase 0 backend amendment from exact Git objects.

This script is an author-provided consistency aid. Its PASS is not an independent
architecture decision; reviewers must inspect the script and derive their own verdict.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys
from typing import Any


EXPECTED_CANDIDATE = "579c2358f5e2245a977abdedcee7e06ba3f4e46e"
EXPECTED_SUBJECT = "Preserve raw store build logs in evidence packets"
EXPECTED_CANDIDATE_MANIFEST = (
    "b43b8b0ea9c3c0c8938363091aaf4de0e7a4a3b3babb225582a85b050a104375"
)
EXPECTED_ARCHITECTURE_COMMIT = "65e60216891bb3d826a4778f84cb8aaa377abe92"
EXPECTED_ARCHITECTURE_MANIFEST = (
    "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0"
)
EXPECTED_SOURCE_MANIFEST = (
    "30be1edac70bb27f78c626bf7099aee1f825f2a0f59797a72b9acdc781e9725e"
)
EXPECTED_PACKETS = [
    {
        "id": "windows-store",
        "manifest_path": (
            "reviews/probes/phase0-store-backend-windows-x64-2026-07-17/"
            "evidence/SHA256SUMS"
        ),
        "manifest_sha256": "6d404cfc4b25ed581a9f021fc6248c6e7a94c2fdc38b668872c009c0f747ef2d",
        "entry_count": 12,
        "summary_path": (
            "reviews/probes/phase0-store-backend-windows-x64-2026-07-17/"
            "evidence/windows-x64-summary.json"
        ),
        "summary_sha256": "6623d3425294bcf8d5949348dd660a9fa9762d7808a453cb0f1b60df9ea385b3",
    },
    {
        "id": "wsl2-store",
        "manifest_path": (
            "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/"
            "evidence/SHA256SUMS"
        ),
        "manifest_sha256": "c65a9224bfd03482ea947661c70edc7168d1662a18eb1a0447604fa58f807b3e",
        "entry_count": 37,
        "summary_path": (
            "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/"
            "evidence/wsl2-ext4-musl-x64-summary.json"
        ),
        "summary_sha256": "f12f91a02c3f15f2525b476c330839351054d23fe52176c91fc9adf28d9ff81f",
    },
    {
        "id": "native-linux-store",
        "manifest_path": (
            "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
            "evidence/SHA256SUMS"
        ),
        "manifest_sha256": "0544d252540e14b8f9392d8a83a37209748af0995d6dd2240397ec4941e75046",
        "entry_count": 37,
        "summary_path": (
            "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
            "evidence/native-linux-ext4-musl-x64-summary.json"
        ),
        "summary_sha256": "49c3a4f7bc7191ee85ae2f3ce15bec35f4bf48a17619f7574598794dd951e5a9",
    },
    {
        "id": "oxc-feasibility",
        "manifest_path": (
            "reviews/probes/phase0-oxc-memory-stack-windows-wsl2-x64-2026-07-17/"
            "evidence/SHA256SUMS"
        ),
        "manifest_sha256": "bfba3524182822ebb9e7ec35c37ae08a1b03380fa0f961675499eef5031790be",
        "entry_count": 80,
        "summary_path": (
            "reviews/probes/phase0-oxc-memory-stack-windows-wsl2-x64-2026-07-17/"
            "evidence/summary.json"
        ),
        "summary_sha256": "2f73daba1fa12b6a518962cab16400faeb275fca08296c02a9ec442f51c9c1c6",
    },
    {
        "id": "backend-selection",
        "manifest_path": (
            "reviews/probes/phase0-store-backend-selection-2026-07-17/SHA256SUMS"
        ),
        "manifest_sha256": "ce14aaab83942e83e6b874d972aef06d03aa03d64c68370cefebac3339287ea6",
        "entry_count": 2,
        "summary_path": (
            "reviews/probes/phase0-store-backend-selection-2026-07-17/selection.json"
        ),
        "summary_sha256": "82a5d5122cd902fc3861fa8d48f831180f2149124ea55e86c318a81f61dbb11b",
    },
]
EXPECTED_NATIVE_RUNNER = {
    "github_run_id": "29584914108",
    "runner_commit": "0b5988c8176c73e9d6d8936cbcc90eebcac3c2a5",
    "artifact_sha256": "9ffc3fd385c1d6b8af748eda20c26f623f4a18420a3e9a540cb91b6f0f7706e4",
}
REQUEST_DIR = Path(__file__).resolve().parent

STORE_SUMMARIES = {
    "windows": (
        "reviews/probes/phase0-store-backend-windows-x64-2026-07-17/"
        "evidence/windows-x64-summary.json"
    ),
    "wsl2": (
        "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/"
        "evidence/wsl2-ext4-musl-x64-summary.json"
    ),
    "native": (
        "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
        "evidence/native-linux-ext4-musl-x64-summary.json"
    ),
}
ADMISSION_ARTIFACTS = (
    "reviews/probes/phase0-store-backend-windows-x64-2026-07-17/"
    "evidence/admission-windows-x64.json",
    "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/"
    "evidence/admission-linux-gnu-ext4-x64.json",
    "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/"
    "evidence/admission-linux-musl-ext4-x64.json",
    "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
    "evidence/admission-linux-gnu-ext4-x64.json",
    "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
    "evidence/admission-linux-musl-ext4-x64.json",
)
FAULT_ARTIFACTS = (
    "reviews/probes/phase0-store-backend-windows-x64-2026-07-17/"
    "evidence/fault-matrix-windows-x64.json",
    "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/"
    "evidence/fault-matrix-linux-gnu-ext4-x64.json",
    "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/"
    "evidence/fault-matrix-linux-musl-ext4-x64.json",
    "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
    "evidence/fault-matrix-linux-gnu-ext4-x64.json",
    "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
    "evidence/fault-matrix-linux-musl-ext4-x64.json",
)
SELECTION_PATH = (
    "reviews/probes/phase0-store-backend-selection-2026-07-17/selection.json"
)
OXC_SUMMARY_PATH = (
    "reviews/probes/phase0-oxc-memory-stack-windows-wsl2-x64-2026-07-17/"
    "evidence/summary.json"
)
NATIVE_PROVENANCE_PATH = (
    "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/"
    "runner-provenance.json"
)


class VerificationError(RuntimeError):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise VerificationError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def decode_json(data: bytes, label: str) -> Any:
    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=reject_duplicate_keys)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise VerificationError(f"invalid JSON in {label}: {error}") from error


def run_git(repo: Path, *args: str, text: bool = False) -> bytes | str:
    process = subprocess.run(
        ["git", *args],
        cwd=repo,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        stderr = process.stderr.decode("utf-8", errors="replace").strip()
        raise VerificationError(f"git {' '.join(args)} failed: {stderr}")
    if text:
        return process.stdout.decode("utf-8").strip()
    return process.stdout


def candidate_blob(repo: Path, path: str) -> bytes:
    return run_git(repo, "cat-file", "blob", f"{EXPECTED_CANDIDATE}:{path}")  # type: ignore[return-value]


def candidate_json(repo: Path, path: str) -> Any:
    return decode_json(candidate_blob(repo, path), path)


def parse_sha_manifest(data: bytes, label: str) -> list[tuple[str, str]]:
    if data.startswith(b"\xef\xbb\xbf") or b"\r" in data or not data.endswith(b"\n"):
        raise VerificationError(f"noncanonical text framing in {label}")
    rows: list[tuple[str, str]] = []
    for line_number, line in enumerate(data.decode("utf-8").splitlines(), start=1):
        digest, separator, path = line.partition("  ")
        if separator != "  " or len(digest) != 64:
            raise VerificationError(f"malformed {label}:{line_number}")
        try:
            int(digest, 16)
        except ValueError as error:
            raise VerificationError(f"non-hex digest in {label}:{line_number}") from error
        if digest != digest.lower():
            raise VerificationError(f"noncanonical digest case in {label}:{line_number}")
        if not path or "\\" in path or "\x00" in path:
            raise VerificationError(f"empty path in {label}:{line_number}")
        rows.append((digest, path))
    paths = [path for _, path in rows]
    if len(paths) != len(set(paths)):
        raise VerificationError(f"duplicate path in {label}")
    return rows


def resolve_manifest_member(manifest_path: str, member: str) -> str:
    member_path = PurePosixPath(member)
    if member_path.is_absolute() or ".." in member_path.parts:
        raise VerificationError(f"unsafe manifest member: {member}")
    return (PurePosixPath(manifest_path).parent / member_path).as_posix()


class Checks:
    def __init__(self) -> None:
        self.rows: list[dict[str, Any]] = []

    def add(
        self,
        name: str,
        passed: bool,
        *,
        observed: Any = None,
        expected: Any = None,
    ) -> None:
        row: dict[str, Any] = {"name": name, "status": "PASS" if passed else "FAIL"}
        if observed is not None:
            row["observed"] = observed
        if expected is not None:
            row["expected"] = expected
        self.rows.append(row)

    @property
    def passed(self) -> bool:
        return all(row["status"] == "PASS" for row in self.rows)


def verify_candidate_identity(repo: Path, checks: Checks) -> list[tuple[str, str]]:
    resolved = run_git(
        repo, "rev-parse", f"{EXPECTED_CANDIDATE}^{{commit}}", text=True
    )
    checks.add(
        "exact-candidate-commit",
        resolved == EXPECTED_CANDIDATE,
        observed=resolved,
        expected=EXPECTED_CANDIDATE,
    )
    subject = run_git(repo, "show", "-s", "--format=%s", EXPECTED_CANDIDATE, text=True)
    checks.add(
        "candidate-commit-subject",
        subject == EXPECTED_SUBJECT,
        observed=subject,
        expected=EXPECTED_SUBJECT,
    )

    local_manifest = (REQUEST_DIR / "candidate-manifest.txt").read_bytes()
    checks.add(
        "request-candidate-manifest-hash",
        sha256(local_manifest) == EXPECTED_CANDIDATE_MANIFEST,
        observed=sha256(local_manifest),
        expected=EXPECTED_CANDIDATE_MANIFEST,
    )
    rows = parse_sha_manifest(local_manifest, "candidate-manifest.txt")
    sorted_paths = sorted((path for _, path in rows), key=lambda path: path.encode("utf-8"))
    checks.add(
        "candidate-manifest-order-and-cardinality",
        [path for _, path in rows] == sorted_paths and len(rows) == 16,
        observed={"entries": len(rows), "sorted": [path for _, path in rows] == sorted_paths},
        expected={"entries": 16, "sorted": True},
    )

    reproduced: list[tuple[str, str]] = []
    failures: list[dict[str, str]] = []
    canonical_bytes: list[bytes] = []
    for expected_digest, path in rows:
        data = candidate_blob(repo, path)
        actual_digest = sha256(data)
        reproduced.append((actual_digest, path))
        canonical_bytes.append(data)
        if actual_digest != expected_digest:
            failures.append(
                {"path": path, "expected": expected_digest, "actual": actual_digest}
            )
    reproduced_bytes = "".join(
        f"{digest}  {path}\n" for digest, path in reproduced
    ).encode("utf-8")
    checks.add(
        "candidate-git-blob-sha256",
        not failures and sha256(reproduced_bytes) == EXPECTED_CANDIDATE_MANIFEST,
        observed={"failures": failures, "manifest_sha256": sha256(reproduced_bytes)},
        expected={"failures": [], "manifest_sha256": EXPECTED_CANDIDATE_MANIFEST},
    )
    canonical_text = all(
        not data.startswith(b"\xef\xbb\xbf") and b"\r" not in data and data.endswith(b"\n")
        for data in canonical_bytes
    )
    checks.add("candidate-text-framing", canonical_text, observed=canonical_text, expected=True)

    artifact_failures: list[str] = []
    for path in (
        "specs/inventory-config-semantics.v1.json",
        "specs/repo-path-semantics.v1.json",
        "specs/resolver-config-semantics.v1.json",
    ):
        try:
            candidate_json(repo, path)
        except VerificationError as error:
            artifact_failures.append(str(error))
    checks.add(
        "machine-artifacts-parse-with-duplicate-key-rejection",
        not artifact_failures,
        observed=artifact_failures,
        expected=[],
    )
    return rows


def verify_packet_manifests(repo: Path, request: dict[str, Any], checks: Checks) -> None:
    total_entries = 0
    wsl_native_log_count = 0
    packet_failures: list[dict[str, Any]] = []
    for packet in request["packets"]:
        manifest_path = packet["manifest_path"]
        manifest_data = candidate_blob(repo, manifest_path)
        actual_manifest_hash = sha256(manifest_data)
        rows = parse_sha_manifest(manifest_data, manifest_path)
        failures: list[dict[str, str]] = []
        for expected_digest, member in rows:
            target = resolve_manifest_member(manifest_path, member)
            actual_digest = sha256(candidate_blob(repo, target))
            if actual_digest != expected_digest:
                failures.append(
                    {"path": target, "expected": expected_digest, "actual": actual_digest}
                )
            if packet["id"] in {"wsl2-store", "native-linux-store"} and target.endswith(
                ".log"
            ):
                wsl_native_log_count += 1
        summary_hash = sha256(candidate_blob(repo, packet["summary_path"]))
        passed = (
            actual_manifest_hash == packet["manifest_sha256"]
            and len(rows) == packet["entry_count"]
            and summary_hash == packet["summary_sha256"]
            and not failures
        )
        total_entries += len(rows)
        checks.add(
            f"packet:{packet['id']}",
            passed,
            observed={
                "manifest_sha256": actual_manifest_hash,
                "entries": len(rows),
                "summary_sha256": summary_hash,
                "member_failures": failures,
            },
            expected={
                "manifest_sha256": packet["manifest_sha256"],
                "entries": packet["entry_count"],
                "summary_sha256": packet["summary_sha256"],
                "member_failures": [],
            },
        )
        if not passed:
            packet_failures.append({"id": packet["id"], "failures": failures})
    checks.add(
        "packet-entry-total",
        total_entries == request["packet_entry_count"],
        observed=total_entries,
        expected=request["packet_entry_count"],
    )
    checks.add(
        "wsl-native-raw-build-logs-present",
        wsl_native_log_count == request["required_raw_build_log_count"],
        observed=wsl_native_log_count,
        expected=request["required_raw_build_log_count"],
    )
    checks.add("packet-failure-total", not packet_failures, observed=packet_failures, expected=[])


def verify_store_correctness(repo: Path, selection: dict[str, Any], checks: Checks) -> None:
    admission_rounds = 0
    forced_deaths = 0
    admission_failures: list[str] = []
    for path in ADMISSION_ARTIFACTS:
        artifact = candidate_json(repo, path)
        if (
            artifact.get("overall_status") != "PASS"
            or artifact.get("architecture_commit") != EXPECTED_ARCHITECTURE_COMMIT
            or artifact.get("source_manifest_sha256") != EXPECTED_SOURCE_MANIFEST
            or artifact.get("rounds") != 32
        ):
            admission_failures.append(path)
            continue
        for backend in artifact.get("backends", []):
            if backend.get("status") != "PASS":
                admission_failures.append(f"{path}:{backend.get('backend')}")
                continue
            admission_rounds += len(backend.get("contention_rounds", []))
            admission_rounds += len(backend.get("disjoint_rounds", []))
            forced_deaths += len(backend.get("crash_cases", []))
    checks.add(
        "store-admission-correctness",
        not admission_failures
        and admission_rounds == selection["correctness"]["admission_contention_rounds"]
        and forced_deaths == selection["correctness"]["forced_admission_death_cases"],
        observed={
            "rounds": admission_rounds,
            "forced_deaths": forced_deaths,
            "failures": admission_failures,
        },
        expected={"rounds": 640, "forced_deaths": 20, "failures": []},
    )

    fault_cases = 0
    fault_failures: list[str] = []
    for path in FAULT_ARTIFACTS:
        artifact = candidate_json(repo, path)
        if artifact.get("overall_status") != "PASS":
            fault_failures.append(path)
            continue
        for backend in artifact.get("backends", []):
            cases = backend.get("cases", [])
            fault_cases += len(cases)
            if backend.get("status") != "PASS" or any(
                case.get("status") != "PASS" for case in cases
            ):
                fault_failures.append(f"{path}:{backend.get('backend')}")
    checks.add(
        "store-fault-matrix-correctness",
        not fault_failures
        and fault_cases == selection["correctness"]["backend_fault_cases"],
        observed={"cases": fault_cases, "failures": fault_failures},
        expected={"cases": 470, "failures": []},
    )


def verify_store_comparison(repo: Path, selection: dict[str, Any], checks: Checks) -> None:
    summaries = {name: candidate_json(repo, path) for name, path in STORE_SUMMARIES.items()}
    summary_consistency = all(
        summary.get("status") == "PASS"
        and summary.get("architecture_commit") == EXPECTED_ARCHITECTURE_COMMIT
        and summary.get("architecture_manifest_sha256") == EXPECTED_ARCHITECTURE_MANIFEST
        and summary.get("source_manifest_sha256") == EXPECTED_SOURCE_MANIFEST
        for summary in summaries.values()
    )
    checks.add("store-summary-identity", summary_consistency, observed=summary_consistency, expected=True)

    namespace_injected = sum(
        summary["correctness"]["namespace_injected_and_detected"]
        for summary in summaries.values()
    )
    namespace_prevented = sum(
        summary["correctness"]["namespace_kernel_prevented_before_displacement"]
        for summary in summaries.values()
    )
    checks.add(
        "namespace-case-totals",
        namespace_injected == selection["correctness"]["namespace_injected_and_detected"]
        and namespace_prevented
        == selection["correctness"]["namespace_kernel_prevented_before_displacement"]
        and namespace_injected + namespace_prevented
        == selection["correctness"]["namespace_cases"],
        observed={"injected": namespace_injected, "kernel_prevented": namespace_prevented},
        expected={"injected": 170, "kernel_prevented": 20},
    )

    groups: dict[tuple[str, str], dict[str, dict[str, Any]]] = {}
    for host, summary in summaries.items():
        for measurement in summary["measurements"]:
            key = (host, measurement.get("mode", "msvc"))
            groups.setdefault(key, {})[measurement["backend"]] = measurement
    pair_shape = len(groups) == 5 and all(set(pair) == {"redb", "sqlite"} for pair in groups.values())
    checks.add(
        "measurement-pair-shape",
        pair_shape,
        observed={"groups": len(groups), "backends": [sorted(pair) for pair in groups.values()]},
        expected={"groups": 5, "backends": [["redb", "sqlite"]] * 5},
    )
    if not pair_shape:
        return

    durable_redb = 0
    binary_redb = 0
    query_redb = 0
    query_sqlite = 0
    rss_redb = 0
    rss_sqlite = 0
    binary_advantages: list[int] = []
    dependency_advantages: list[int] = []
    redb_store: set[int] = set()
    sqlite_store: set[int] = set()
    redb_native_sources: set[int] = set()
    sqlite_native_sources: set[int] = set()
    for pair in groups.values():
        redb = pair["redb"]
        sqlite = pair["sqlite"]
        durable_redb += redb["durable_admission"]["p50_micros"] < sqlite["durable_admission"]["p50_micros"]
        binary_redb += redb["binary_bytes"] < sqlite["binary_bytes"]
        query_redb += redb["warm_reopen_query"]["p50_micros"] < sqlite["warm_reopen_query"]["p50_micros"]
        query_sqlite += sqlite["warm_reopen_query"]["p50_micros"] < redb["warm_reopen_query"]["p50_micros"]
        rss_redb += redb["peak_working_set_bytes"] < sqlite["peak_working_set_bytes"]
        rss_sqlite += sqlite["peak_working_set_bytes"] < redb["peak_working_set_bytes"]
        binary_advantages.append(sqlite["binary_bytes"] - redb["binary_bytes"])
        dependency_advantages.append(
            sqlite["transitive_package_count"] - redb["transitive_package_count"]
        )
        redb_store.add(redb["store_bytes"])
        sqlite_store.add(sqlite["store_bytes"])
        redb_native_sources.add(redb["native_source_bytes"])
        sqlite_native_sources.add(sqlite["native_source_bytes"])

    observed = {
        "durable_redb": durable_redb,
        "binary_redb": binary_redb,
        "query_redb": query_redb,
        "query_sqlite": query_sqlite,
        "rss_redb": rss_redb,
        "rss_sqlite": rss_sqlite,
        "binary_advantage_min": min(binary_advantages),
        "binary_advantage_max": max(binary_advantages),
        "dependency_advantages": sorted(set(dependency_advantages)),
        "redb_store": sorted(redb_store),
        "sqlite_store": sorted(sqlite_store),
        "redb_native_source_bytes": sorted(redb_native_sources),
        "sqlite_native_source_bytes": sorted(sqlite_native_sources),
    }
    expected = {
        "durable_redb": 5,
        "binary_redb": 5,
        "query_redb": 1,
        "query_sqlite": 4,
        "rss_redb": 0,
        "rss_sqlite": 5,
        "binary_advantage_min": 913408,
        "binary_advantage_max": 1299776,
        "dependency_advantages": [12, 13],
        "redb_store": [8425472],
        "sqlite_store": [3182592],
        "redb_native_source_bytes": [0],
        "sqlite_native_source_bytes": [20340001],
    }
    selection_comparison = selection["comparison"]
    selection_matches = (
        selection_comparison["durable_admission_p50_winner"]["scopes_won"] == durable_redb
        and selection_comparison["release_binary_winner"]["scopes_won"] == binary_redb
        and selection_comparison["bounded_query_p50"]["redb_scopes_won"] == query_redb
        and selection_comparison["bounded_query_p50"]["sqlite_scopes_won"] == query_sqlite
        and selection_comparison["peak_rss"]["redb_scopes_won"] == rss_redb
        and selection_comparison["peak_rss"]["sqlite_scopes_won"] == rss_sqlite
        and selection_comparison["release_binary_winner"]["observed_byte_advantage_min"]
        == min(binary_advantages)
        and selection_comparison["release_binary_winner"]["observed_byte_advantage_max"]
        == max(binary_advantages)
        and selection_comparison["store_bytes"]["redb"] == next(iter(redb_store))
        and selection_comparison["store_bytes"]["sqlite"] == next(iter(sqlite_store))
    )
    checks.add(
        "measured-backend-comparison",
        observed == expected and selection_matches,
        observed={"derived": observed, "selection_matches": selection_matches},
        expected={"derived": expected, "selection_matches": True},
    )


def verify_selection_and_oxc(repo: Path, request: dict[str, Any], checks: Checks) -> None:
    selection = candidate_json(repo, SELECTION_PATH)
    selection_identity = (
        selection.get("schema") == "lumin-phase0-store-backend-selection-v1"
        and selection.get("status") == "PASS"
        and selection.get("architecture_commit") == EXPECTED_ARCHITECTURE_COMMIT
        and selection.get("architecture_manifest_sha256") == EXPECTED_ARCHITECTURE_MANIFEST
        and selection.get("probe_source_manifest_sha256") == EXPECTED_SOURCE_MANIFEST
        and selection.get("selected_backend", {}).get("crate") == "redb"
        and selection.get("selected_backend", {}).get("version") == "4.1.0"
        and selection.get("rejected_production_backend", {}).get("crate") == "rusqlite"
        and selection.get("rejected_production_backend", {}).get("version") == "0.39.0"
        and selection.get("rejected_production_backend", {}).get("sqlite") == "bundled"
        and selection.get("correctness", {}).get("candidate_failures") == 0
    )
    checks.add("selection-identity", selection_identity, observed=selection_identity, expected=True)

    packet_by_id = {packet["id"]: packet for packet in request["packets"]}
    expected_evidence = {
        "windows-x64-ntfs": packet_by_id["windows-store"],
        "wsl2-ext4-gnu-musl-x64": packet_by_id["wsl2-store"],
        "native-linux-ext4-gnu-musl-x64": packet_by_id["native-linux-store"],
    }
    observed_evidence = {item["scope"]: item for item in selection.get("evidence", [])}
    evidence_bindings = set(observed_evidence) == set(expected_evidence) and all(
        observed_evidence[scope].get("evidence_manifest_sha256")
        == packet["manifest_sha256"]
        and observed_evidence[scope].get("summary_sha256") == packet["summary_sha256"]
        for scope, packet in expected_evidence.items()
    )
    checks.add(
        "selection-evidence-bindings",
        evidence_bindings,
        observed=observed_evidence,
        expected={
            scope: {
                "evidence_manifest_sha256": packet["manifest_sha256"],
                "summary_sha256": packet["summary_sha256"],
            }
            for scope, packet in expected_evidence.items()
        },
    )

    remaining_gate_text = "\n".join(selection.get("remaining_phase0_gates", []))
    required_gate_terms = (
        "package",
        "public process-reopen",
        "upstream provenance",
        "numeric Phase 1",
        "independent review",
    )
    remaining_gates_preserved = all(term in remaining_gate_text for term in required_gate_terms)
    checks.add(
        "selection-remaining-gates-preserved",
        remaining_gates_preserved,
        observed=selection.get("remaining_phase0_gates", []),
        expected=list(required_gate_terms),
    )
    verify_store_correctness(repo, selection, checks)
    verify_store_comparison(repo, selection, checks)

    clauses = {
        "architecture/000-system-blueprint.md": (
            "exact `redb 4.1.0` as the sole production persistence engine",
            "any production persistence dependency other than exact `redb 4.1.0`",
        ),
        "architecture/002-evidence-and-write-gate.md": (
            "Architecture v1 selects exact `redb 4.1.0` as the sole production backend.",
            "Bundled SQLite remains probe-only evidence",
        ),
        "specs/001-foundation-slice.md": (
            "`lumin-store` uses exact `redb 4.1.0`",
            "The Slice has no second backend feature, runtime selector, fallback database",
        ),
    }
    missing_clauses: list[str] = []
    for path, required in clauses.items():
        text = candidate_blob(repo, path).decode("utf-8")
        for clause in required:
            if clause not in text:
                missing_clauses.append(f"{path}: {clause}")
    checks.add("canonical-redb-selection-clauses", not missing_clauses, observed=missing_clauses, expected=[])

    gitignore = candidate_blob(repo, ".gitignore").decode("utf-8")
    log_exceptions = (
        "!reviews/probes/**/evidence/*.log" in gitignore
        and "!reviews/probes/**/evidence/**/*.log" in gitignore
    )
    checks.add("probe-log-ignore-exceptions", log_exceptions, observed=log_exceptions, expected=True)

    oxc = candidate_json(repo, OXC_SUMMARY_PATH)
    oxc_platforms = oxc.get("platforms", [])
    oxc_boundary = (
        oxc.get("status") == "PASS"
        and oxc.get("conclusion", {}).get("numeric_budget") == "NOT_APPROVED"
        and oxc.get("conclusion", {}).get("native_linux_evidence") == "PENDING"
        and len(oxc_platforms) == 2
        and all(platform.get("minimum_observed_passing_stack_bytes") == 1048576 for platform in oxc_platforms)
    )
    checks.add("oxc-feasibility-boundary", oxc_boundary, observed=oxc_boundary, expected=True)

    provenance = candidate_json(repo, NATIVE_PROVENANCE_PATH)
    requested_runner = request["native_runner"]
    provenance_consistency = (
        provenance.get("github_run_id") == requested_runner["github_run_id"]
        and provenance.get("github_sha") == requested_runner["runner_commit"]
        and selection["evidence"][2].get("github_run_id") == requested_runner["github_run_id"]
        and selection["evidence"][2].get("github_runner_commit")
        == requested_runner["runner_commit"]
        and selection["evidence"][2].get("github_artifact_sha256")
        == requested_runner["artifact_sha256"]
    )
    checks.add(
        "native-runner-provenance-consistency",
        provenance_consistency,
        observed=provenance_consistency,
        expected=True,
    )


def load_request(checks: Checks) -> dict[str, Any]:
    request_path = REQUEST_DIR / "evidence-packets.json"
    request = decode_json(request_path.read_bytes(), request_path.name)
    identity = (
        request.get("schema") == "lumin-phase0-backend-independent-review-request-v1"
        and request.get("candidate_commit") == EXPECTED_CANDIDATE
        and request.get("candidate_commit_message") == EXPECTED_SUBJECT
        and request.get("candidate_manifest_sha256") == EXPECTED_CANDIDATE_MANIFEST
        and request.get("architecture_baseline_commit") == EXPECTED_ARCHITECTURE_COMMIT
        and request.get("architecture_baseline_manifest_sha256")
        == EXPECTED_ARCHITECTURE_MANIFEST
        and request.get("probe_source_manifest_sha256") == EXPECTED_SOURCE_MANIFEST
        and request.get("packet_entry_count") == 168
        and request.get("required_raw_build_log_count") == 20
        and request.get("packets") == EXPECTED_PACKETS
        and request.get("native_runner") == EXPECTED_NATIVE_RUNNER
    )
    checks.add("request-identity", identity, observed=identity, expected=True)
    return request


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".", help="Git checkout containing the candidate object")
    parser.add_argument("--output", help="Optional JSON result path")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    checks = Checks()
    try:
        request = load_request(checks)
        origin = run_git(repo, "remote", "get-url", "origin", text=True)
        verify_candidate_identity(repo, checks)
        verify_packet_manifests(repo, request, checks)
        verify_selection_and_oxc(repo, request, checks)
        result = {
            "schema": "lumin-phase0-backend-independent-review-preflight-v1",
            "status": "PASS" if checks.passed else "FAIL",
            "candidate_commit": EXPECTED_CANDIDATE,
            "candidate_manifest_sha256": EXPECTED_CANDIDATE_MANIFEST,
            "origin": origin,
            "checks_passed": sum(row["status"] == "PASS" for row in checks.rows),
            "checks_total": len(checks.rows),
            "checks": checks.rows,
            "limitations": [
                "This author-provided verifier does not establish reviewer independence.",
                "The GitHub Actions artifact digest is cross-checked against committed metadata but is not downloaded here.",
                "Package, public-behavior, upstream-provenance, and numeric-budget gates remain outside this amendment review.",
            ],
        }
    except (OSError, VerificationError, KeyError, TypeError, ValueError) as error:
        result = {
            "schema": "lumin-phase0-backend-independent-review-preflight-v1",
            "status": "FAIL",
            "candidate_commit": EXPECTED_CANDIDATE,
            "candidate_manifest_sha256": EXPECTED_CANDIDATE_MANIFEST,
            "checks_passed": sum(row["status"] == "PASS" for row in checks.rows),
            "checks_total": len(checks.rows),
            "checks": checks.rows,
            "fatal_error": str(error),
        }
    payload = json.dumps(result, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    if args.output:
        Path(args.output).write_text(payload, encoding="utf-8", newline="\n")
    sys.stdout.write(payload)
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
