#!/usr/bin/env python3
"""Author-side consistency verifier for NEW-STATIC-PACKAGING-01 closure."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from typing import Any
import zipfile


PREVIOUS = "e0a2810b46f6091895b5e9f7dd4454e8854fee0e"
CANDIDATE = "4315eb7dee35fff3de40fb04e1dd3c4a3fc990e3"
PARENT = "bb79655cb94c59822a928ede1b861980e6358b6a"
TREE = "2cb4e5e055e8e9e82e26351af5ad3ddc2ca40a11"
SUBJECT = "Regenerate exact-bound static packaging evidence"
BINDING_COMMIT = "56f7e17c1e45d3538604336365105541db39ada3"
SOURCE_COMMIT = "bb79655cb94c59822a928ede1b861980e6358b6a"
PACKET_PREFIX = (
    "reviews/probes/"
    "phase0-static-packaging-windows-wsl2-native-linux-x64-2026-07-18"
)
PACKET_MANIFEST_SHA256 = (
    "ad2e746441ee778ecf8e8f51a12a331d3c6b3c78a1c995fb661970ab925b6764"
)
SOURCE_MANIFEST_SHA256 = (
    "38c1a75d06edb12bb2798d93bc1ce788325ca33c6bc12dabd4ef10df943b677c"
)
ARCHITECTURE_CANDIDATE = "9a0dbe5c89463892c001e864c4f18eeab9e0eaed"
ARCHITECTURE_MANIFEST_SHA256 = (
    "e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a"
)
NATIVE_ARCHIVE_SHA256 = (
    "2f238899ccccbb43a1c345eab3746f68da56a86208ef0d46fa11e36853cbb971"
)
NATIVE_ARCHIVE_SIZE = 1_659_125
NATIVE_RUN_ID = "29634512936"
NATIVE_RUNNER_COMMIT = "b7560b443d973540020bd2de984a99b69c35d14e"
NATIVE_ARTIFACT_ID = "8426637860"
EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()
REQUEST_DIR = Path(__file__).resolve().parent

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

SCOPES = {
    "windows-ntfs": {
        "count": 13,
        "manifest": "bbbe8ac057f70f0993073d237a9d17715a92810f7635d58d47bca381a64e1b01",
        "labels": ("windows-msvc",),
    },
    "wsl2-ext4": {
        "count": 21,
        "manifest": "fefc42bc6b06fce1f5dd7804559348b068e6f4fda52006326b65abbbc19ed1bc",
        "labels": ("linux-gnu", "linux-musl"),
    },
    "native-linux-ext4": {
        "count": 21,
        "manifest": "6c8c8f22b9aaa0e8104417a542de0bd2e02386173d3b54d8d2cccdd97e947339",
        "labels": ("linux-gnu", "linux-musl"),
    },
}

DETACHED = {
    "windows-msvc": {
        "relative": "windows-msvc.exe",
        "sha256": "dd7ba4cda6e5654f864c79c18e0aa0a9a96001b0591490e3038d416a11762d7a",
        "size": 1_412_608,
        "scope": "windows-ntfs",
    },
    "wsl2-linux-gnu": {
        "relative": "wsl2-linux-gnu",
        "sha256": "6892e467d61fc2ffcc3c0fec73323a8d8c2d789e709cfd801d0ff94ffc50caf4",
        "size": 1_794_736,
        "scope": "wsl2-ext4",
        "label": "linux-gnu",
    },
    "wsl2-linux-musl": {
        "relative": "wsl2-linux-musl",
        "sha256": "ad9b7d8789111ede6c065805185d21cc07a87400555eedecad859952fb258a32",
        "size": 1_897_184,
        "scope": "wsl2-ext4",
        "label": "linux-musl",
    },
    "native-linux-gnu": {
        "zip_member": (
            "source/target/x86_64-unknown-linux-gnu/release/"
            "lumin-phase0-static-packaging-probe"
        ),
        "sha256": "07c708408353f15956ebaecc8e58b196f0ee7af70a9b1f974c9d2a0d4825d69f",
        "size": 1_794_800,
        "scope": "native-linux-ext4",
        "label": "linux-gnu",
    },
    "native-linux-musl": {
        "zip_member": (
            "source/target/x86_64-unknown-linux-musl/release/"
            "lumin-phase0-static-packaging-probe"
        ),
        "sha256": "6ca7bb97f794a631f6375848de2f74c5f2444a7ea27086e09a2b4fec67270563",
        "size": 1_901_280,
        "scope": "native-linux-ext4",
        "label": "linux-musl",
    },
}


class VerificationError(RuntimeError):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def run_git(repo: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise VerificationError(
            f"git {' '.join(args)} failed: "
            f"{result.stderr.decode(errors='replace').strip()}"
        )
    return result.stdout


def git_text(repo: Path, *args: str) -> str:
    return run_git(repo, *args).decode("utf-8").strip()


def git_blob(repo: Path, revision: str, path: str) -> bytes:
    return run_git(repo, "cat-file", "blob", f"{revision}:{path}")


def strict_json(data: bytes, label: str) -> Any:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise VerificationError(f"duplicate JSON key in {label}: {key}")
            result[key] = value
        return result

    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=reject_duplicates)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise VerificationError(f"invalid JSON in {label}: {exc}") from exc


def parse_manifest(data: bytes, label: str, count: int) -> list[tuple[str, str]]:
    if data.startswith(b"\xef\xbb\xbf") or b"\r" in data or not data.endswith(b"\n"):
        raise VerificationError(f"noncanonical manifest framing: {label}")
    entries: list[tuple[str, str]] = []
    for line in data.decode("utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            raise VerificationError(f"bad manifest line in {label}: {line!r}")
        relative = match.group(2)
        pure = PurePosixPath(relative)
        if pure.is_absolute() or ".." in pure.parts or "\\" in relative or ":" in relative:
            raise VerificationError(f"unsafe manifest path in {label}: {relative!r}")
        entries.append((match.group(1), relative))
    paths = [path for _, path in entries]
    if len(paths) != count or len(set(paths)) != count:
        raise VerificationError(f"wrong manifest cardinality in {label}: {len(paths)}")
    if paths != sorted(paths, key=lambda value: value.encode("utf-8")):
        raise VerificationError(f"manifest is not UTF-8 ordinal sorted: {label}")
    return entries


def verify_git_manifest(
    repo: Path,
    base: str,
    expected_sha: str,
    count: int,
) -> tuple[bytes, list[tuple[str, str]]]:
    manifest_path = f"{base}/SHA256SUMS"
    data = git_blob(repo, CANDIDATE, manifest_path)
    if sha256(data) != expected_sha:
        raise VerificationError(f"manifest digest mismatch: {manifest_path}")
    entries = parse_manifest(data, manifest_path, count)
    for expected, relative in entries:
        member_path = f"{base}/{relative}"
        if sha256(git_blob(repo, CANDIDATE, member_path)) != expected:
            raise VerificationError(f"manifest member mismatch: {member_path}")
    tree_paths = {
        path
        for path in git_text(
            repo, "ls-tree", "-r", "--name-only", CANDIDATE, base
        ).splitlines()
        if path
    }
    expected_paths = {f"{base}/{relative}" for _, relative in entries}
    expected_paths.add(manifest_path)
    if tree_paths != expected_paths:
        raise VerificationError(f"manifest inventory mismatch under {base}")
    return data, entries


def root_dependencies(metadata: dict[str, Any]) -> dict[str, str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    root = metadata["resolve"]["root"]
    node = next(item for item in metadata["resolve"]["nodes"] if item["id"] == root)
    return {
        packages[dependency["pkg"]]["name"]: packages[dependency["pkg"]]["version"]
        for dependency in node["deps"]
    }


def c_string(data: bytes, offset: int, description: str) -> str:
    if offset < 0 or offset >= len(data):
        raise VerificationError(f"{description} offset is outside artifact")
    end = data.find(b"\0", offset)
    if end < 0:
        raise VerificationError(f"{description} is not NUL terminated")
    try:
        return data[offset:end].decode("utf-8")
    except UnicodeError as exc:
        raise VerificationError(f"{description} is not UTF-8") from exc


def inspect_pe(data: bytes) -> dict[str, Any]:
    if len(data) < 64 or data[:2] != b"MZ":
        raise VerificationError("Windows artifact is not MZ")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 24 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise VerificationError("Windows artifact has no PE signature")
    machine, section_count = struct.unpack_from("<HH", data, pe_offset + 4)
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    optional_offset = pe_offset + 24
    if optional_offset + optional_size > len(data):
        raise VerificationError("Windows optional header is truncated")
    magic = struct.unpack_from("<H", data, optional_offset)[0]
    if machine != 0x8664 or magic != 0x20B:
        raise VerificationError("Windows artifact is not PE32+ x86-64")
    sections: list[tuple[int, int, int, int]] = []
    sections_offset = optional_offset + optional_size
    for index in range(section_count):
        offset = sections_offset + index * 40
        if offset + 40 > len(data):
            raise VerificationError("Windows section table is truncated")
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", data, offset + 8
        )
        if raw_offset + raw_size > len(data):
            raise VerificationError("Windows section data is truncated")
        sections.append((virtual_address, max(virtual_size, raw_size), raw_offset, raw_size))

    def rva_to_offset(rva: int) -> int:
        for virtual_address, span, raw_offset, raw_size in sections:
            if virtual_address <= rva < virtual_address + span:
                delta = rva - virtual_address
                if delta < raw_size:
                    return raw_offset + delta
        raise VerificationError(f"Windows RVA {rva:#x} is not file-backed")

    imports: list[str] = []
    if optional_size >= 128:
        import_rva, import_size = struct.unpack_from("<II", data, optional_offset + 120)
        if import_rva and import_size:
            offset = rva_to_offset(import_rva)
            end = min(len(data), offset + import_size)
            while offset + 20 <= end:
                descriptor = struct.unpack_from("<IIIII", data, offset)
                if descriptor == (0, 0, 0, 0, 0):
                    break
                imports.append(c_string(data, rva_to_offset(descriptor[3]), "PE import"))
                offset += 20
    return {
        "format": "PE32+-x86_64",
        "interpreter": None,
        "machine": "x86_64",
        "neededLibraries": sorted(set(imports), key=str.casefold),
        "static": not imports,
    }


def inspect_elf(data: bytes) -> dict[str, Any]:
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01":
        raise VerificationError("artifact is not little-endian ELF64")
    if struct.unpack_from("<H", data, 18)[0] != 62:
        raise VerificationError("ELF artifact is not x86-64")
    program_offset = struct.unpack_from("<Q", data, 32)[0]
    entry_size = struct.unpack_from("<H", data, 54)[0]
    count = struct.unpack_from("<H", data, 56)[0]
    if entry_size < 56:
        raise VerificationError("ELF program-header entry is too small")
    programs: list[dict[str, int]] = []
    for index in range(count):
        offset = program_offset + index * entry_size
        if offset + 56 > len(data):
            raise VerificationError("ELF program-header table is truncated")
        values = struct.unpack_from("<IIQQQQQQ", data, offset)
        program = {
            "type": values[0],
            "offset": values[2],
            "vaddr": values[3],
            "filesz": values[5],
        }
        if program["offset"] + program["filesz"] > len(data):
            raise VerificationError("ELF program segment is truncated")
        programs.append(program)

    interpreter = None
    for program in programs:
        if program["type"] == 3:
            raw = data[program["offset"] : program["offset"] + program["filesz"]]
            if not raw.endswith(b"\0"):
                raise VerificationError("ELF interpreter is not NUL terminated")
            interpreter = raw[:-1].decode("utf-8")
            break

    needed_offsets: list[int] = []
    string_vaddr = None
    string_size = None
    for program in programs:
        if program["type"] != 2:
            continue
        dynamic_end = program["offset"] + program["filesz"]
        for offset in range(program["offset"], dynamic_end - 15, 16):
            tag, value = struct.unpack_from("<qQ", data, offset)
            if tag == 0:
                break
            if tag == 1:
                needed_offsets.append(value)
            elif tag == 5:
                string_vaddr = value
            elif tag == 10:
                string_size = value

    needed: list[str] = []
    if needed_offsets:
        if string_vaddr is None or string_size is None:
            raise VerificationError("ELF dynamic string table is missing")
        string_offset = None
        for program in programs:
            if (
                program["type"] == 1
                and program["vaddr"] <= string_vaddr
                and string_vaddr < program["vaddr"] + program["filesz"]
            ):
                string_offset = program["offset"] + string_vaddr - program["vaddr"]
                break
        if string_offset is None or string_offset + string_size > len(data):
            raise VerificationError("ELF dynamic string table is not file-backed")
        for needed_offset in needed_offsets:
            if needed_offset >= string_size:
                raise VerificationError("ELF DT_NEEDED offset is outside string table")
            needed.append(c_string(data, string_offset + needed_offset, "ELF needed"))
    return {
        "format": "ELF64-x86_64",
        "interpreter": interpreter,
        "machine": "x86_64",
        "neededLibraries": needed,
        "static": interpreter is None and not needed,
    }


def expected_run(label: str) -> dict[str, Any]:
    if label == "windows-msvc":
        os_name, target_env = "windows", "msvc"
    elif label == "linux-gnu":
        os_name, target_env = "linux", "gnu"
    else:
        os_name, target_env = "linux", "musl"
    return {
        "schema": "lumin-phase0-static-packaging-run-v2",
        "status": "PASS",
        "architectureCandidate": ARCHITECTURE_CANDIDATE,
        "architectureManifestSha256": ARCHITECTURE_MANIFEST_SHA256,
        "sourceManifestSha256": SOURCE_MANIFEST_SHA256,
        "os": os_name,
        "arch": "x86_64",
        "targetEnv": target_env,
        "oxcStatementCount": 2,
        "rayonSum": 4950,
        "redbValue": 42,
    }


def execute_bytes(data: bytes, suffix: str) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="lumin-static-closure-") as directory:
        path = Path(directory) / f"probe{suffix}"
        path.write_bytes(data)
        path.chmod(0o755)
        result = subprocess.run(
            [str(path)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False
        )
    if result.returncode != 0 or result.stderr:
        raise VerificationError(
            f"detached artifact execution failed: rc={result.returncode} "
            f"stderr={result.stderr.decode(errors='replace')!r}"
        )
    return strict_json(result.stdout, "fresh detached artifact stdout")


def verify_evidence(repo: Path, check: Any) -> dict[str, dict[str, Any]]:
    summaries: dict[str, dict[str, Any]] = {}
    packet_data, packet_entries = verify_git_manifest(
        repo, PACKET_PREFIX, PACKET_MANIFEST_SHA256, 73
    )
    check("packet-manifest", len(packet_entries) == 73, "73/73 exact Git blobs")
    check(
        "request-packet-manifest-copy",
        (REQUEST_DIR / "packet-manifest.txt").read_bytes() == packet_data,
        PACKET_MANIFEST_SHA256,
    )
    _, source_entries = verify_git_manifest(
        repo, f"{PACKET_PREFIX}/source", SOURCE_MANIFEST_SHA256, 9
    )
    check("source-manifest", len(source_entries) == 9, "9/9 exact source blobs")

    for _, relative in packet_entries:
        if relative.endswith(".json"):
            strict_json(
                git_blob(repo, CANDIDATE, f"{PACKET_PREFIX}/{relative}"), relative
            )
    check("packet-json-strict-parse", True, "all packet JSON rejects duplicate keys")

    for scope, spec in SCOPES.items():
        base = f"{PACKET_PREFIX}/evidence/{scope}"
        _, entries = verify_git_manifest(
            repo, base, spec["manifest"], spec["count"]
        )
        check(f"evidence-manifest:{scope}", len(entries) == spec["count"], str(spec["count"]))
        summary = strict_json(
            git_blob(repo, CANDIDATE, f"{base}/summary.json"), f"{scope}/summary"
        )
        summaries[scope] = summary
        labels = tuple(item["label"] for item in summary["artifacts"])
        check(
            f"summary-binding:{scope}",
            summary["schema"] == "lumin-phase0-static-packaging-summary-v2"
            and summary["status"] == "PASS"
            and summary["scope"] == scope
            and summary["sourceFileCount"] == 9
            and summary["sourceManifestSha256"] == SOURCE_MANIFEST_SHA256
            and summary["architectureCandidate"] == ARCHITECTURE_CANDIDATE
            and summary["architectureManifestSha256"] == ARCHITECTURE_MANIFEST_SHA256
            and labels == spec["labels"],
            repr(labels),
        )
        boundary = summary["claimBoundary"]
        check(
            f"claim-boundary:{scope}",
            boundary == {
                "achievedProductBudgets": False,
                "nativePathRootDto": False,
                "packagedSkills": False,
                "productApiOrScaffold": False,
                "publicProcessBehavior": False,
                "staticPackagingFeasibilityOnly": True,
            },
            "standalone feasibility only",
        )
        metadata = strict_json(
            git_blob(repo, CANDIDATE, f"{base}/cargo-metadata.json"),
            f"{scope}/cargo-metadata",
        )
        links = sorted(
            f"{package['name']}@{package['version']}:{package['links']}"
            for package in metadata["packages"]
            if package.get("links")
        )
        check(
            f"dependencies:{scope}",
            root_dependencies(metadata) == EXPECTED_DEPENDENCIES,
            "exact direct versions",
        )
        check(
            f"cargo-links:{scope}",
            links == ["rayon-core@1.13.0:rayon-core"]
            and summary["unexpectedCargoLinkDeclarations"] == [],
            repr(links),
        )
        controls = strict_json(
            git_blob(repo, CANDIDATE, f"{base}/negative-controls.json"),
            f"{scope}/negative-controls",
        )
        by_id = {item["id"]: item for item in controls["controls"]}
        expected_ids = {
            "tampered-source-identity",
            "unrelated-native-executable",
            "pre-existing-run-output",
            "dynamic-gnu-labeled-musl",
        }
        linux = scope != "windows-ntfs"
        check(
            f"negative-controls:{scope}",
            controls["status"] == "PASS"
            and set(by_id) == expected_ids
            and all(
                by_id[item]["observed"] == "REJECTED"
                for item in expected_ids - {"dynamic-gnu-labeled-musl"}
            )
            and by_id["pre-existing-run-output"]["observedRejectionCode"]
            == "stale-generated-output"
            and by_id["dynamic-gnu-labeled-musl"]["observed"]
            == ("REJECTED" if linux else "NOT_APPLICABLE")
            and (
                not linux
                or by_id["dynamic-gnu-labeled-musl"]["observedRejectionCode"]
                == "dynamic-musl-artifact"
            ),
            "four exact controls",
        )
        summary_artifacts = {item["label"]: item for item in summary["artifacts"]}
        for label in spec["labels"]:
            run_data = git_blob(repo, CANDIDATE, f"{base}/run-{label}.json")
            run = strict_json(run_data, f"{scope}/run-{label}")
            inspection = strict_json(
                git_blob(repo, CANDIDATE, f"{base}/inspection-{label}.json"),
                f"{scope}/inspection-{label}",
            )
            execution = strict_json(
                git_blob(repo, CANDIDATE, f"{base}/execution-{label}.json"),
                f"{scope}/execution-{label}",
            )
            artifact = summary_artifacts[label]
            check(f"run-v2:{scope}/{label}", run == expected_run(label), "exact run contract")
            check(
                f"inspection-binding:{scope}/{label}",
                inspection["schema"] == "lumin-phase0-static-packaging-inspection-v1"
                and inspection["label"] == label
                and all(
                    inspection[key] == artifact[key]
                    for key in (
                        "format",
                        "interpreter",
                        "machine",
                        "neededLibraries",
                        "static",
                    )
                ),
                artifact["sha256"],
            )
            check(
                f"execution-binding:{scope}/{label}",
                execution["schema"] == "lumin-phase0-static-packaging-execution-v1"
                and execution["status"] == "PASS"
                and execution["label"] == label
                and execution["exitCode"] == 0
                and execution["artifactSizeBytes"] == artifact["sizeBytes"]
                and execution["runJsonSha256"] == sha256(run_data)
                and execution["stderrSha256"] == EMPTY_SHA256
                and all(
                    execution[key] == artifact["sha256"]
                    for key in (
                        "artifactSha256",
                        "artifactSha256AfterExecution",
                        "executionCopySha256Before",
                        "executionCopySha256After",
                    )
                ),
                artifact["sha256"],
            )
    return summaries


def verify_artifacts(
    repo: Path,
    artifact_root: Path,
    summaries: dict[str, dict[str, Any]],
    check: Any,
) -> None:
    root = artifact_root.resolve(strict=True)
    native_zip = root / "native-workflow.zip"
    native_bytes = native_zip.read_bytes()
    check(
        "native-workflow-archive",
        len(native_bytes) == NATIVE_ARCHIVE_SIZE
        and sha256(native_bytes) == NATIVE_ARCHIVE_SHA256,
        f"{len(native_bytes)} {sha256(native_bytes)}",
    )
    detached: dict[str, bytes] = {}
    for name, spec in DETACHED.items():
        if "relative" in spec:
            detached[name] = (root / spec["relative"]).read_bytes()
    with zipfile.ZipFile(native_zip) as archive:
        names = set(archive.namelist())
        native_manifest_name = "evidence/native-linux-ext4/SHA256SUMS"
        manifest_data = archive.read(native_manifest_name)
        entries = parse_manifest(manifest_data, native_manifest_name, 21)
        expected_names = {native_manifest_name}
        for expected, relative in entries:
            name = f"evidence/native-linux-ext4/{relative}"
            expected_names.add(name)
            member = archive.read(name)
            if sha256(member) != expected:
                raise VerificationError(f"native ZIP member hash mismatch: {name}")
            committed = git_blob(
                repo,
                CANDIDATE,
                f"{PACKET_PREFIX}/evidence/native-linux-ext4/{relative}",
            )
            if member != committed:
                raise VerificationError(f"native ZIP/Git evidence mismatch: {name}")
        for name, spec in DETACHED.items():
            if "zip_member" in spec:
                expected_names.add(spec["zip_member"])
                detached[name] = archive.read(spec["zip_member"])
        if names != expected_names:
            raise VerificationError("native ZIP has missing or extra members")
    check("native-zip-evidence", True, "21/21 Git-identical evidence members")

    for name, spec in DETACHED.items():
        data = detached[name]
        label = spec.get("label", "windows-msvc")
        check(
            f"detached-identity:{name}",
            len(data) == spec["size"] and sha256(data) == spec["sha256"],
            f"{len(data)} {sha256(data)}",
        )
        inspection = inspect_pe(data) if label == "windows-msvc" else inspect_elf(data)
        if label == "linux-musl" and not inspection["static"]:
            raise VerificationError(f"dynamic artifact labeled musl: {name}")
        if label == "linux-gnu" and inspection["static"]:
            raise VerificationError(f"static artifact labeled GNU: {name}")
        summary_artifact = next(
            item
            for item in summaries[spec["scope"]]["artifacts"]
            if item["label"] == label
        )
        check(
            f"detached-inspection:{name}",
            all(
                inspection[key] == summary_artifact[key]
                for key in (
                    "format",
                    "interpreter",
                    "machine",
                    "neededLibraries",
                    "static",
                )
            ),
            inspection["format"],
        )

    if os.name == "nt":
        fresh = execute_bytes(detached["windows-msvc"], ".exe")
        check("fresh-run:windows-msvc", fresh == expected_run("windows-msvc"), "exact stdout")
    elif sys.platform.startswith("linux"):
        for name in (
            "wsl2-linux-gnu",
            "wsl2-linux-musl",
            "native-linux-gnu",
            "native-linux-musl",
        ):
            label = DETACHED[name]["label"]
            fresh = execute_bytes(detached[name], "")
            check(f"fresh-run:{name}", fresh == expected_run(label), "exact stdout")

        script = repo / PACKET_PREFIX / "source/scripts/package_evidence.py"
        source = repo / PACKET_PREFIX / "source"
        evidence = repo / PACKET_PREFIX / "evidence/wsl2-ext4"
        true_path = Path("/bin/true")
        if true_path.is_file():
            substituted = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    "verify",
                    "--source",
                    str(source),
                    "--evidence",
                    str(evidence),
                    "--artifact",
                    f"linux-gnu={true_path}",
                    "--artifact",
                    f"linux-musl={true_path}",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            check(
                "negative-control:unrelated-gnu",
                substituted.returncode != 0 and b"run-json-invalid" in substituted.stderr,
                substituted.stderr.decode(errors="replace").strip(),
            )
            dynamic_musl = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    "inspect",
                    "--artifact",
                    f"linux-musl={true_path}",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            check(
                "negative-control:dynamic-musl",
                dynamic_musl.returncode != 0
                and b"dynamic-musl-artifact" in dynamic_musl.stderr,
                dynamic_musl.stderr.decode(errors="replace").strip(),
            )
        stale = subprocess.run(
            [
                sys.executable,
                str(script),
                "seal",
                "--scope",
                "wsl2-ext4",
                "--source",
                str(source),
                "--evidence",
                str(evidence),
                "--artifact",
                f"linux-gnu={root / 'wsl2-linux-gnu'}",
                "--artifact",
                f"linux-musl={root / 'wsl2-linux-musl'}",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        check(
            "negative-control:stale-generated-output",
            stale.returncode != 0 and b"stale-generated-output" in stale.stderr,
            stale.stderr.decode(errors="replace").strip(),
        )


def verify(repo: Path, artifact_root: Path | None) -> dict[str, Any]:
    results: list[dict[str, str]] = []

    def check(name: str, condition: bool, evidence: str) -> None:
        if not condition:
            raise VerificationError(f"{name}: {evidence}")
        results.append({"name": name, "status": "PASS", "evidence": evidence})

    check("candidate-object", git_text(repo, "cat-file", "-t", CANDIDATE) == "commit", CANDIDATE)
    metadata = git_text(repo, "show", "-s", "--format=%P%n%T%n%s", CANDIDATE).splitlines()
    check("candidate-parent", metadata[0] == PARENT, metadata[0])
    check("candidate-tree", metadata[1] == TREE, metadata[1])
    check("candidate-subject", metadata[2] == SUBJECT, metadata[2])
    for name, commit in (("binding-commit", BINDING_COMMIT), ("source-commit", SOURCE_COMMIT)):
        ancestor = subprocess.run(
            ["git", "-C", str(repo), "merge-base", "--is-ancestor", commit, CANDIDATE],
            check=False,
        )
        check(name, ancestor.returncode == 0, commit)
    candidate_paths = git_text(repo, "diff", "--name-only", PARENT, CANDIDATE).splitlines()
    check(
        "candidate-path-scope",
        bool(candidate_paths)
        and all(path.startswith(f"{PACKET_PREFIX}/") for path in candidate_paths),
        f"{len(candidate_paths)} probe-only paths",
    )
    range_paths = git_text(repo, "diff", "--name-only", PREVIOUS, CANDIDATE).splitlines()
    check(
        "closure-range-scope",
        bool(range_paths) and all(path.startswith("reviews/") for path in range_paths),
        f"{len(range_paths)} review/probe-only paths",
    )

    architecture_data = (REQUEST_DIR / "architecture-manifest.txt").read_bytes()
    check(
        "architecture-manifest-digest",
        sha256(architecture_data) == ARCHITECTURE_MANIFEST_SHA256,
        sha256(architecture_data),
    )
    architecture_entries = parse_manifest(architecture_data, "architecture-manifest.txt", 16)
    for expected, path in architecture_entries:
        if sha256(git_blob(repo, CANDIDATE, path)) != expected:
            raise VerificationError(f"architecture blob mismatch: {path}")
    check("architecture-binding", True, "16/16 exact candidate blobs")
    for _, path in architecture_entries:
        if path.endswith(".json"):
            strict_json(git_blob(repo, CANDIDATE, path), path)
    check("architecture-machine-artifacts", True, "3/3 strict JSON")

    summaries = verify_evidence(repo, check)
    native_checks = strict_json(
        git_blob(repo, CANDIDATE, f"{PACKET_PREFIX}/runner/native-independent-checks.json"),
        "native-independent-checks",
    )
    check(
        "native-workflow-record",
        native_checks["status"] == "PASS"
        and native_checks["checkCount"] == 72
        and native_checks["workflowRunId"] == NATIVE_RUN_ID
        and native_checks["runnerCommit"] == NATIVE_RUNNER_COMMIT
        and native_checks["artifactId"] == NATIVE_ARTIFACT_ID
        and native_checks["artifactArchiveSha256"] == NATIVE_ARCHIVE_SHA256,
        "72/72 retained workflow-local checks",
    )

    request = strict_json((REQUEST_DIR / "closure-request.json").read_bytes(), "closure-request")
    check(
        "request-binding",
        request["candidate"] == CANDIDATE
        and request["candidate_parent"] == PARENT
        and request["candidate_tree"] == TREE
        and request["packet_manifest"]["sha256"] == PACKET_MANIFEST_SHA256
        and request["source_manifest"]["sha256"] == SOURCE_MANIFEST_SHA256,
        "exact closure identities",
    )

    if artifact_root is not None:
        verify_artifacts(repo, artifact_root, summaries, check)

    return {
        "schema": "lumin-new-static-packaging-01-author-preflight-v1",
        "status": "PASS",
        "candidate": CANDIDATE,
        "packetManifestSha256": PACKET_MANIFEST_SHA256,
        "sourceManifestSha256": SOURCE_MANIFEST_SHA256,
        "artifactRootChecked": artifact_root is not None,
        "checks": {"pass": len(results), "fail": 0},
        "results": results,
        "independenceBoundary": (
            "Author-side consistency output only; not independent review evidence."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--artifact-root", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        result = verify(args.repo.resolve(), args.artifact_root)
    except (
        OSError,
        VerificationError,
        ValueError,
        KeyError,
        StopIteration,
        zipfile.BadZipFile,
    ) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        return 1
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8", newline="\n")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
