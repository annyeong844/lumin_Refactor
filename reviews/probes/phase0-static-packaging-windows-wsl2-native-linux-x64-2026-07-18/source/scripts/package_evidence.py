#!/usr/bin/env python3
"""Seal and verify exact-artifact Phase 0 static-packaging evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import struct
import subprocess
import sys
import tempfile
from typing import Any


ARCHITECTURE_CANDIDATE = "9a0dbe5c89463892c001e864c4f18eeab9e0eaed"
ARCHITECTURE_MANIFEST = "e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a"
RUN_SCHEMA = "lumin-phase0-static-packaging-run-v2"
INSPECTION_SCHEMA = "lumin-phase0-static-packaging-inspection-v1"
EXECUTION_SCHEMA = "lumin-phase0-static-packaging-execution-v1"
NEGATIVE_CONTROL_SCHEMA = "lumin-phase0-static-packaging-negative-controls-v1"
SUMMARY_SCHEMA = "lumin-phase0-static-packaging-summary-v2"
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
EXPECTED_CARGO_LINKS = ["rayon-core@1.13.0:rayon-core"]
EXPECTED_RUNS = {
    "windows-msvc": ("windows", "msvc"),
    "linux-gnu": ("linux", "gnu"),
    "linux-musl": ("linux", "musl"),
}
EXPECTED_SCOPES = {
    "windows-ntfs": {
        "filesystem": "ntfs",
        "hostKind": "windows",
        "labels": {"windows-msvc"},
        "os": "windows",
    },
    "wsl2-ext4": {
        "filesystem": "ext4",
        "hostKind": "wsl2",
        "labels": {"linux-gnu", "linux-musl"},
        "os": "linux",
    },
    "native-linux-ext4": {
        "filesystem": "ext4",
        "hostKind": "native-linux",
        "labels": {"linux-gnu", "linux-musl"},
        "os": "linux",
    },
}
CLAIM_BOUNDARY = {
    "achievedProductBudgets": False,
    "nativePathRootDto": False,
    "packagedSkills": False,
    "productApiOrScaffold": False,
    "publicProcessBehavior": False,
    "staticPackagingFeasibilityOnly": True,
}
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")


class EvidenceError(RuntimeError):
    def __init__(self, message: str, *, code: str = "evidence-invalid") -> None:
        super().__init__(message)
        self.code = code


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def strict_json_bytes(data: bytes, description: str) -> Any:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise EvidenceError(
                    f"duplicate JSON key in {description}: {key}",
                    code="duplicate-json-key",
                )
            result[key] = value
        return result

    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=reject_duplicates)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise EvidenceError(
            f"invalid JSON {description}: {exc}", code="run-json-invalid"
        ) from exc


def strict_json(path: Path) -> Any:
    try:
        return strict_json_bytes(path.read_bytes(), str(path))
    except OSError as exc:
        raise EvidenceError(f"cannot read JSON {path}: {exc}") from exc


def validate_relative_path(value: str, manifest: Path) -> None:
    pure = PurePosixPath(value)
    if (
        not value
        or "\\" in value
        or ":" in value
        or pure.is_absolute()
        or any(part in {"", ".", ".."} for part in pure.parts)
    ):
        raise EvidenceError(f"unsafe manifest path in {manifest}: {value!r}")


def parse_manifest(path: Path) -> list[tuple[str, str]]:
    data = path.read_bytes()
    if data.startswith(b"\xef\xbb\xbf") or b"\r" in data or not data.endswith(b"\n"):
        raise EvidenceError(f"noncanonical manifest framing: {path}")
    entries: list[tuple[str, str]] = []
    for line in data.decode("utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            raise EvidenceError(f"bad manifest line in {path}: {line!r}")
        validate_relative_path(match.group(2), path)
        entries.append((match.group(1), match.group(2)))
    names = [name for _, name in entries]
    if len(names) != len(set(names)):
        raise EvidenceError(f"duplicate manifest path in {path}")
    if names != sorted(names, key=lambda value: value.encode("utf-8")):
        raise EvidenceError(f"manifest is not UTF-8 ordinal sorted: {path}")
    return entries


def listed_files(root: Path, manifest: Path, *, source: bool) -> set[str]:
    files = set()
    for path in root.rglob("*"):
        if not path.is_file() or path == manifest:
            continue
        relative = path.relative_to(root)
        if source and any(
            part == "target" or part.startswith("target-") for part in relative.parts
        ):
            continue
        files.add(relative.as_posix())
    return files


def verify_manifest(root: Path, manifest: Path, *, source: bool) -> int:
    root = root.resolve(strict=True)
    entries = parse_manifest(manifest)
    listed = set()
    for expected, relative in entries:
        target = (root / Path(relative)).resolve(strict=True)
        target.relative_to(root)
        if not target.is_file() or target.is_symlink():
            raise EvidenceError(f"manifest target is not a regular file: {target}")
        actual = sha256(target.read_bytes())
        if actual != expected:
            raise EvidenceError(f"manifest mismatch for {relative}: {actual}")
        listed.add(relative)
    actual_files = listed_files(root, manifest, source=source)
    if actual_files != listed:
        raise EvidenceError(
            f"manifest inventory mismatch: missing={sorted(listed - actual_files)!r} "
            f"extra={sorted(actual_files - listed)!r}"
        )
    return len(entries)


def verify_source(source: Path) -> dict[str, Any]:
    source = source.resolve(strict=True)
    manifest = source / "SHA256SUMS"
    count = verify_manifest(source, manifest, source=True)
    return {
        "sourceFileCount": count,
        "sourceManifestSha256": sha256(manifest.read_bytes()),
        "status": "PASS",
    }


def parse_artifact(value: str) -> tuple[str, Path]:
    if "=" not in value:
        raise argparse.ArgumentTypeError("artifact must be LABEL=PATH")
    label, raw_path = value.split("=", 1)
    if label not in EXPECTED_RUNS:
        raise argparse.ArgumentTypeError(f"unknown artifact label: {label}")
    return label, Path(raw_path).resolve()


def c_string(data: bytes, offset: int, description: str) -> str:
    if offset < 0 or offset >= len(data):
        raise EvidenceError(f"{description} string offset is outside the artifact")
    end = data.find(b"\0", offset)
    if end < 0:
        raise EvidenceError(f"{description} string is not NUL terminated")
    try:
        return data[offset:end].decode("utf-8")
    except UnicodeError as exc:
        raise EvidenceError(f"{description} string is not UTF-8") from exc


def inspect_pe(data: bytes) -> dict[str, Any]:
    if len(data) < 64 or data[:2] != b"MZ":
        raise EvidenceError("Windows artifact is not an MZ executable", code="format-mismatch")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 24 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise EvidenceError("Windows artifact has no PE signature", code="format-mismatch")
    machine, section_count = struct.unpack_from("<HH", data, pe_offset + 4)
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    optional_offset = pe_offset + 24
    if optional_offset + optional_size > len(data):
        raise EvidenceError("Windows optional header is truncated", code="format-mismatch")
    optional_magic = struct.unpack_from("<H", data, optional_offset)[0]
    if machine != 0x8664 or optional_magic != 0x20B:
        raise EvidenceError(
            f"Windows artifact is not PE32+ x86-64: machine={machine:#x} "
            f"magic={optional_magic:#x}",
            code="format-mismatch",
        )
    sections_offset = optional_offset + optional_size
    sections: list[tuple[int, int, int, int]] = []
    for index in range(section_count):
        offset = sections_offset + index * 40
        if offset + 40 > len(data):
            raise EvidenceError("Windows section table is truncated", code="format-mismatch")
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", data, offset + 8
        )
        if raw_offset + raw_size > len(data):
            raise EvidenceError("Windows section data is truncated", code="format-mismatch")
        sections.append((virtual_address, max(virtual_size, raw_size), raw_offset, raw_size))

    def rva_to_offset(rva: int) -> int:
        for virtual_address, span, raw_offset, raw_size in sections:
            if virtual_address <= rva < virtual_address + span:
                delta = rva - virtual_address
                if delta >= raw_size:
                    break
                return raw_offset + delta
        raise EvidenceError(f"Windows RVA {rva:#x} is not file-backed", code="format-mismatch")

    imports: list[str] = []
    data_directory_offset = optional_offset + 112
    if optional_size >= 128:
        import_rva, import_size = struct.unpack_from("<II", data, data_directory_offset + 8)
        if import_rva and import_size:
            descriptor_offset = rva_to_offset(import_rva)
            descriptor_end = min(len(data), descriptor_offset + import_size)
            while descriptor_offset + 20 <= descriptor_end:
                descriptor = struct.unpack_from("<IIIII", data, descriptor_offset)
                if descriptor == (0, 0, 0, 0, 0):
                    break
                imports.append(c_string(data, rva_to_offset(descriptor[3]), "PE import"))
                descriptor_offset += 20
    imports = sorted(set(imports), key=lambda value: value.casefold())
    return {
        "format": "PE32+-x86_64",
        "interpreter": None,
        "machine": "x86_64",
        "neededLibraries": imports,
        "schema": INSPECTION_SCHEMA,
        "static": not imports,
    }


def inspect_elf(data: bytes) -> dict[str, Any]:
    if (
        len(data) < 64
        or data[:4] != b"\x7fELF"
        or data[4] != 2
        or data[5] != 1
    ):
        raise EvidenceError("artifact is not little-endian ELF64", code="format-mismatch")
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != 62:
        raise EvidenceError(
            f"ELF artifact machine is not x86-64: {machine}", code="format-mismatch"
        )
    program_offset = struct.unpack_from("<Q", data, 32)[0]
    program_entry_size = struct.unpack_from("<H", data, 54)[0]
    program_count = struct.unpack_from("<H", data, 56)[0]
    if program_entry_size < 56:
        raise EvidenceError("ELF program-header entry is too small", code="format-mismatch")
    programs: list[dict[str, int]] = []
    for index in range(program_count):
        offset = program_offset + index * program_entry_size
        if offset + 56 > len(data):
            raise EvidenceError("ELF program-header table is truncated", code="format-mismatch")
        values = struct.unpack_from("<IIQQQQQQ", data, offset)
        program = {
            "type": values[0],
            "offset": values[2],
            "vaddr": values[3],
            "filesz": values[5],
            "memsz": values[6],
        }
        if program["offset"] + program["filesz"] > len(data):
            raise EvidenceError("ELF program segment is truncated", code="format-mismatch")
        programs.append(program)

    interpreter = None
    for program in programs:
        if program["type"] == 3:
            raw = data[program["offset"] : program["offset"] + program["filesz"]]
            if not raw.endswith(b"\0"):
                raise EvidenceError("ELF interpreter is not NUL terminated", code="format-mismatch")
            interpreter = raw[:-1].decode("utf-8")
            break

    needed_offsets: list[int] = []
    string_table_vaddr = None
    string_table_size = None
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
                string_table_vaddr = value
            elif tag == 10:
                string_table_size = value

    needed: list[str] = []
    if needed_offsets:
        if string_table_vaddr is None or string_table_size is None:
            raise EvidenceError("ELF dynamic string table is missing", code="format-mismatch")
        string_table_offset = None
        for program in programs:
            if (
                program["type"] == 1
                and program["vaddr"] <= string_table_vaddr
                and string_table_vaddr < program["vaddr"] + program["filesz"]
            ):
                string_table_offset = (
                    program["offset"] + string_table_vaddr - program["vaddr"]
                )
                break
        if string_table_offset is None:
            raise EvidenceError("ELF dynamic string table is not file-backed", code="format-mismatch")
        if string_table_offset + string_table_size > len(data):
            raise EvidenceError("ELF dynamic string table is truncated", code="format-mismatch")
        for needed_offset in needed_offsets:
            if needed_offset >= string_table_size:
                raise EvidenceError("ELF DT_NEEDED offset is outside the string table")
            needed.append(c_string(data, string_table_offset + needed_offset, "ELF needed"))
    return {
        "format": "ELF64-x86_64",
        "interpreter": interpreter,
        "machine": "x86_64",
        "neededLibraries": needed,
        "schema": INSPECTION_SCHEMA,
        "static": interpreter is None and not needed,
    }


def inspect_artifact(label: str, data: bytes) -> dict[str, Any]:
    inspection = inspect_pe(data) if label == "windows-msvc" else inspect_elf(data)
    inspection = {"label": label, **inspection}
    if label == "windows-msvc" and inspection["format"] != "PE32+-x86_64":
        raise EvidenceError("Windows artifact format mismatch", code="format-mismatch")
    if label.startswith("linux-") and inspection["format"] != "ELF64-x86_64":
        raise EvidenceError("Linux artifact format mismatch", code="format-mismatch")
    if label == "linux-musl" and not inspection["static"]:
        raise EvidenceError(
            "musl artifact has an interpreter or dynamic dependency",
            code="dynamic-musl-artifact",
        )
    if label == "linux-gnu" and inspection["static"]:
        raise EvidenceError(
            "GNU artifact lacks the expected dynamic interpreter/dependencies",
            code="gnu-linkage-mismatch",
        )
    return inspection


def expected_run(label: str, source_manifest_sha256: str) -> dict[str, Any]:
    expected_os, expected_env = EXPECTED_RUNS[label]
    return {
        "arch": "x86_64",
        "architectureCandidate": ARCHITECTURE_CANDIDATE,
        "architectureManifestSha256": ARCHITECTURE_MANIFEST,
        "os": expected_os,
        "oxcStatementCount": 2,
        "rayonSum": 4950,
        "redbValue": 42,
        "schema": RUN_SCHEMA,
        "sourceManifestSha256": source_manifest_sha256,
        "status": "PASS",
        "targetEnv": expected_env,
    }


def validate_run_output(
    stdout: bytes, stderr: bytes, returncode: int, label: str, source_manifest_sha256: str
) -> dict[str, Any]:
    if returncode != 0:
        raise EvidenceError(
            f"exact artifact execution failed for {label}: exit {returncode}",
            code="artifact-execution-failed",
        )
    if stderr:
        raise EvidenceError(
            f"exact artifact wrote stderr for {label}", code="artifact-stderr-nonempty"
        )
    run = strict_json_bytes(stdout, f"{label} exact-artifact stdout")
    expected = expected_run(label, source_manifest_sha256)
    if run != expected:
        raise EvidenceError(
            f"unexpected exact-artifact run result for {label}: {run!r}",
            code="run-contract-mismatch",
        )
    return run


def execute_copy(data: bytes, label: str) -> subprocess.CompletedProcess[bytes]:
    digest = sha256(data)
    suffix = ".exe" if label == "windows-msvc" else ""
    with tempfile.TemporaryDirectory(prefix="lumin-static-seal-") as directory:
        executable = Path(directory) / f"artifact-{digest}{suffix}"
        executable.write_bytes(data)
        executable.chmod(stat.S_IRUSR | stat.S_IXUSR)
        try:
            if sha256(executable.read_bytes()) != digest:
                raise EvidenceError("execution copy changed before invocation", code="artifact-race")
            completed = subprocess.run(
                [str(executable)],
                cwd=directory,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            if sha256(executable.read_bytes()) != digest:
                raise EvidenceError("execution copy changed during invocation", code="artifact-race")
            return completed
        finally:
            if os.name == "nt" and executable.exists():
                executable.chmod(stat.S_IREAD | stat.S_IWRITE)


def collect_artifact(
    label: str, binary: Path, source_manifest_sha256: str
) -> dict[str, Any]:
    if not binary.is_file() or binary.is_symlink():
        raise EvidenceError(f"artifact missing or not regular: {binary}")
    data = binary.read_bytes()
    artifact_digest = sha256(data)
    inspection = inspect_artifact(label, data)
    completed = execute_copy(data, label)
    run = validate_run_output(
        completed.stdout,
        completed.stderr,
        completed.returncode,
        label,
        source_manifest_sha256,
    )
    if sha256(binary.read_bytes()) != artifact_digest:
        raise EvidenceError("source artifact changed during sealing", code="artifact-race")
    execution = {
        "artifactSha256": artifact_digest,
        "artifactSha256AfterExecution": artifact_digest,
        "artifactSizeBytes": len(data),
        "executionCopySha256After": artifact_digest,
        "executionCopySha256Before": artifact_digest,
        "exitCode": completed.returncode,
        "label": label,
        "runJsonSha256": sha256(completed.stdout),
        "schema": EXECUTION_SCHEMA,
        "status": "PASS",
        "stderrSha256": sha256(completed.stderr),
    }
    summary = {
        "format": inspection["format"],
        "interpreter": inspection["interpreter"],
        "label": label,
        "machine": inspection["machine"],
        "neededLibraries": inspection["neededLibraries"],
        "sha256": artifact_digest,
        "sizeBytes": len(data),
        "static": inspection["static"],
    }
    return {
        "execution": execution,
        "inspection": inspection,
        "run": run,
        "runBytes": completed.stdout,
        "stderrBytes": completed.stderr,
        "summary": summary,
    }


def refuse_generated_outputs(evidence: Path, labels: set[str]) -> None:
    names = {"negative-controls.json", "SHA256SUMS", "summary.json"}
    for label in labels:
        names.update(
            {
                f"execution-{label}.json",
                f"inspection-{label}.json",
                f"linkage-{label}.txt",
                f"run-{label}.json",
                f"run-{label}.stderr.log",
            }
        )
    stale = sorted(name for name in names if (evidence / name).exists())
    if stale:
        raise EvidenceError(
            f"refusing pre-existing generated evidence: {stale!r}",
            code="stale-generated-output",
        )


def expect_rejection(
    identifier: str, expected_codes: str | set[str], operation: Any
) -> dict[str, Any]:
    allowed = {expected_codes} if isinstance(expected_codes, str) else expected_codes
    try:
        operation()
    except EvidenceError as exc:
        if exc.code not in allowed:
            raise EvidenceError(
                f"negative control {identifier} rejected for {exc.code}, "
                f"expected one of {sorted(allowed)!r}"
            ) from exc
        return {
            "expectedRejectionCodes": sorted(allowed),
            "id": identifier,
            "observed": "REJECTED",
            "observedRejectionCode": exc.code,
        }
    raise EvidenceError(f"negative control was accepted: {identifier}")


def negative_controls(
    artifacts_by_label: dict[str, Path], source_manifest_sha256: str
) -> dict[str, Any]:
    first_label = sorted(artifacts_by_label)[0]
    first_data = artifacts_by_label[first_label].read_bytes()
    source_bytes = source_manifest_sha256.encode("ascii")
    if source_bytes not in first_data:
        raise EvidenceError("source-manifest identity is not embedded in the artifact")
    tampered = first_data.replace(source_bytes, b"0" * 64)

    def reject_tampered_identity() -> None:
        inspect_artifact(first_label, tampered)
        completed = execute_copy(tampered, first_label)
        validate_run_output(
            completed.stdout,
            completed.stderr,
            completed.returncode,
            first_label,
            source_manifest_sha256,
        )

    controls = [
        expect_rejection(
            "tampered-source-identity",
            {"artifact-execution-failed", "run-contract-mismatch"},
            reject_tampered_identity,
        )
    ]

    native_label = "windows-msvc" if os.name == "nt" else "linux-gnu"
    foreign_artifact = (
        Path(os.environ["SystemRoot"]) / "System32" / "where.exe"
        if os.name == "nt"
        else Path("/bin/true")
    )

    def reject_unrelated_native_executable() -> None:
        collect_artifact(native_label, foreign_artifact, source_manifest_sha256)

    try:
        reject_unrelated_native_executable()
    except EvidenceError as exc:
        controls.append(
            {
                "expected": "any hard rejection before authorization",
                "id": "unrelated-native-executable",
                "observed": "REJECTED",
                "observedRejectionCode": exc.code,
            }
        )
    else:
        raise EvidenceError("unrelated native executable was authorized")

    with tempfile.TemporaryDirectory(prefix="lumin-static-stale-") as directory:
        stale_root = Path(directory)
        (stale_root / f"run-{first_label}.json").write_text("{}\n", encoding="utf-8")
        controls.append(
            expect_rejection(
                "pre-existing-run-output",
                "stale-generated-output",
                lambda: refuse_generated_outputs(stale_root, {first_label}),
            )
        )

    if "linux-gnu" in artifacts_by_label:
        gnu_data = artifacts_by_label["linux-gnu"].read_bytes()
        controls.append(
            expect_rejection(
                "dynamic-gnu-labeled-musl",
                "dynamic-musl-artifact",
                lambda: inspect_artifact("linux-musl", gnu_data),
            )
        )
    else:
        controls.append(
            {
                "id": "dynamic-gnu-labeled-musl",
                "observed": "NOT_APPLICABLE",
            }
        )
    return {
        "controls": controls,
        "schema": NEGATIVE_CONTROL_SCHEMA,
        "status": "PASS",
    }


def root_dependency_versions(metadata: dict[str, Any]) -> dict[str, str]:
    packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages, list) or not isinstance(resolve, dict):
        raise EvidenceError("Cargo metadata packages or resolve graph missing")
    root_id = resolve.get("root")
    nodes = resolve.get("nodes")
    if not isinstance(root_id, str) or not isinstance(nodes, list):
        raise EvidenceError("Cargo metadata root node missing")
    root_node = next((node for node in nodes if node.get("id") == root_id), None)
    if not isinstance(root_node, dict) or not isinstance(root_node.get("deps"), list):
        raise EvidenceError("Cargo metadata root dependencies missing")
    package_by_id = {package.get("id"): package for package in packages}
    observed: dict[str, str] = {}
    for dependency in root_node["deps"]:
        package = package_by_id.get(dependency.get("pkg"))
        if not isinstance(package, dict):
            raise EvidenceError("Cargo metadata dependency package missing")
        name = package.get("name")
        version = package.get("version")
        if not isinstance(name, str) or not isinstance(version, str) or name in observed:
            raise EvidenceError(f"invalid or duplicate direct dependency: {name!r}")
        observed[name] = version
    if observed != EXPECTED_DEPENDENCIES:
        raise EvidenceError(f"direct dependency version mismatch: {observed!r}")
    return observed


def cargo_link_declarations(metadata: dict[str, Any]) -> list[str]:
    linked = []
    for package in metadata.get("packages", []):
        if package.get("links"):
            linked.append(f"{package.get('name')}@{package.get('version')}:{package['links']}")
    linked.sort()
    if linked != EXPECTED_CARGO_LINKS:
        raise EvidenceError(f"Cargo links surface differs from the frozen oracle: {linked!r}")
    return linked


def validate_host(host: dict[str, Any], scope: str, source_manifest_sha256: str) -> None:
    spec = EXPECTED_SCOPES.get(scope)
    if spec is None:
        raise EvidenceError(f"unknown evidence scope: {scope}")
    expected = {
        "arch": "x86_64",
        "filesystemType": spec["filesystem"],
        "hostKind": spec["hostKind"],
        "os": spec["os"],
        "schema": "lumin-phase0-static-packaging-host-v1",
        "scope": scope,
    }
    for key, value in expected.items():
        if host.get(key) != value:
            raise EvidenceError(f"host {key} mismatch for {scope}: {host.get(key)!r}")
    for key in ("filesystemDetail", "sourcePath", "rustcVersion", "rustcVerbose", "cargoVersion"):
        if not isinstance(host.get(key), str) or not host[key]:
            raise EvidenceError(f"host identity field missing: {key}")
    if host.get("sourceManifestSha256") != source_manifest_sha256:
        raise EvidenceError("host source-manifest identity mismatch")
    if not host["rustcVersion"].startswith("rustc 1.96.0 "):
        raise EvidenceError(f"wrong rustc version: {host['rustcVersion']}")
    if not host["cargoVersion"].startswith("cargo 1.96.0 "):
        raise EvidenceError(f"wrong cargo version: {host['cargoVersion']}")


def validate_raw_files(evidence: Path, labels: set[str]) -> None:
    for label in labels:
        build_stdout = evidence / f"build-{label}.stdout.log"
        build_stderr = evidence / f"build-{label}.stderr.log"
        tree = evidence / f"cargo-tree-{label}.txt"
        for path in (build_stdout, build_stderr, tree):
            if not path.is_file():
                raise EvidenceError(f"raw evidence missing: {path}")
        build_text = build_stderr.read_text(encoding="utf-8", errors="replace")
        if "Finished `release` profile" not in build_text:
            raise EvidenceError(f"release build completion missing: {build_stderr}")
        if tree.stat().st_size == 0:
            raise EvidenceError(f"Cargo tree is empty: {tree}")


def write_json(path: Path, value: Any) -> None:
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def artifact_map(values: list[tuple[str, Path]], scope: str) -> dict[str, Path]:
    spec = EXPECTED_SCOPES.get(scope)
    if spec is None:
        raise EvidenceError(f"unknown scope: {scope}")
    result = dict(values)
    if len(result) != len(values) or set(result) != spec["labels"]:
        raise EvidenceError(
            f"artifact labels for {scope} must be exactly {sorted(spec['labels'])!r}"
        )
    return result


def write_generated_artifact(evidence: Path, collected: dict[str, Any]) -> None:
    label = collected["summary"]["label"]
    (evidence / f"run-{label}.json").write_bytes(collected["runBytes"])
    (evidence / f"run-{label}.stderr.log").write_bytes(collected["stderrBytes"])
    write_json(evidence / f"inspection-{label}.json", collected["inspection"])
    write_json(evidence / f"execution-{label}.json", collected["execution"])


def seal(args: argparse.Namespace) -> dict[str, Any]:
    source = args.source.resolve(strict=True)
    evidence = args.evidence.resolve(strict=True)
    source_identity = verify_source(source)
    artifacts_by_label = artifact_map(args.artifact, args.scope)
    refuse_generated_outputs(evidence, set(artifacts_by_label))

    metadata = strict_json(evidence / "cargo-metadata.json")
    if not isinstance(metadata, dict):
        raise EvidenceError("Cargo metadata must be an object")
    dependencies = root_dependency_versions(metadata)
    linked = cargo_link_declarations(metadata)
    host = strict_json(evidence / "host.json")
    if not isinstance(host, dict):
        raise EvidenceError("host identity must be an object")
    validate_host(host, args.scope, source_identity["sourceManifestSha256"])
    validate_raw_files(evidence, set(artifacts_by_label))

    collected = [
        collect_artifact(label, binary, source_identity["sourceManifestSha256"])
        for label, binary in sorted(artifacts_by_label.items())
    ]
    controls = negative_controls(
        artifacts_by_label, source_identity["sourceManifestSha256"]
    )
    for result in collected:
        write_generated_artifact(evidence, result)
    write_json(evidence / "negative-controls.json", controls)

    summary = {
        "architectureCandidate": ARCHITECTURE_CANDIDATE,
        "architectureManifestSha256": ARCHITECTURE_MANIFEST,
        "artifacts": [result["summary"] for result in collected],
        "cargoLinkDeclarations": linked,
        "cargoLinkInterpretation": {
            "rayon-core@1.13.0:rayon-core": "non-native one-version uniqueness sentinel"
        },
        "claimBoundary": CLAIM_BOUNDARY,
        "directDependencies": dependencies,
        "host": host,
        "negativeControls": controls,
        "schema": SUMMARY_SCHEMA,
        "scope": args.scope,
        "unexpectedCargoLinkDeclarations": [],
        **source_identity,
    }
    write_json(evidence / "summary.json", summary)

    manifest_path = evidence / "SHA256SUMS"
    files = sorted(
        (path for path in evidence.rglob("*") if path.is_file() and path != manifest_path),
        key=lambda path: path.relative_to(evidence).as_posix().encode("utf-8"),
    )
    lines = [
        f"{sha256(path.read_bytes())}  {path.relative_to(evidence).as_posix()}"
        for path in files
    ]
    manifest_path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    verify(evidence, source, args.artifact)
    return summary


def verify(
    evidence: Path, source: Path, artifact_values: list[tuple[str, Path]]
) -> dict[str, Any]:
    evidence = evidence.resolve(strict=True)
    source_identity = verify_source(source)
    count = verify_manifest(evidence, evidence / "SHA256SUMS", source=False)
    summary = strict_json(evidence / "summary.json")
    if not isinstance(summary, dict):
        raise EvidenceError("summary must be an object")
    scope = summary.get("scope")
    if not isinstance(scope, str):
        raise EvidenceError("summary scope is missing")
    artifacts_by_label = artifact_map(artifact_values, scope)
    controls = strict_json(evidence / "negative-controls.json")
    if not isinstance(controls, dict) or controls.get("status") != "PASS":
        raise EvidenceError("negative-control record is invalid")
    if (
        summary.get("schema") != SUMMARY_SCHEMA
        or summary.get("status") != "PASS"
        or summary.get("architectureCandidate") != ARCHITECTURE_CANDIDATE
        or summary.get("architectureManifestSha256") != ARCHITECTURE_MANIFEST
        or summary.get("claimBoundary") != CLAIM_BOUNDARY
        or summary.get("cargoLinkDeclarations") != EXPECTED_CARGO_LINKS
        or summary.get("cargoLinkInterpretation")
        != {
            "rayon-core@1.13.0:rayon-core": "non-native one-version uniqueness sentinel"
        }
        or summary.get("unexpectedCargoLinkDeclarations") != []
        or summary.get("sourceFileCount") != source_identity["sourceFileCount"]
        or summary.get("sourceManifestSha256") != source_identity["sourceManifestSha256"]
        or summary.get("negativeControls") != controls
    ):
        raise EvidenceError("summary contract or source identity mismatch")
    host = strict_json(evidence / "host.json")
    if summary.get("host") != host or not isinstance(host, dict):
        raise EvidenceError("summary host identity mismatch")
    validate_host(host, scope, source_identity["sourceManifestSha256"])
    metadata = strict_json(evidence / "cargo-metadata.json")
    if not isinstance(metadata, dict):
        raise EvidenceError("Cargo metadata must be an object")
    dependencies = root_dependency_versions(metadata)
    links = cargo_link_declarations(metadata)
    if summary.get("cargoLinkDeclarations") != links:
        raise EvidenceError("summary Cargo links declaration mismatch")
    if summary.get("directDependencies") != dependencies:
        raise EvidenceError("summary direct dependency mismatch")
    validate_raw_files(evidence, set(artifacts_by_label))

    rerun = [
        collect_artifact(label, binary, source_identity["sourceManifestSha256"])
        for label, binary in sorted(artifacts_by_label.items())
    ]
    if summary.get("artifacts") != [result["summary"] for result in rerun]:
        raise EvidenceError("summary artifact identity differs from exact supplied bytes")
    for result in rerun:
        label = result["summary"]["label"]
        if (evidence / f"run-{label}.json").read_bytes() != result["runBytes"]:
            raise EvidenceError(f"retained run output is stale for {label}")
        if (evidence / f"run-{label}.stderr.log").read_bytes() != result["stderrBytes"]:
            raise EvidenceError(f"retained stderr is stale for {label}")
        if strict_json(evidence / f"inspection-{label}.json") != result["inspection"]:
            raise EvidenceError(f"retained binary inspection is stale for {label}")
        if strict_json(evidence / f"execution-{label}.json") != result["execution"]:
            raise EvidenceError(f"retained execution binding is stale for {label}")

    rerun_controls = negative_controls(
        artifacts_by_label, source_identity["sourceManifestSha256"]
    )
    recorded_control_ids = {
        item.get("id") for item in controls.get("controls", []) if isinstance(item, dict)
    }
    rerun_control_ids = {
        item.get("id")
        for item in rerun_controls.get("controls", [])
        if isinstance(item, dict)
    }
    if recorded_control_ids != rerun_control_ids or rerun_controls.get("status") != "PASS":
        raise EvidenceError("negative controls did not reproduce")
    return {"evidenceFiles": count, "status": "PASS", "summary": summary}


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    source_parser = subparsers.add_parser("verify-source")
    source_parser.add_argument("--source", type=Path, required=True)

    inspect_parser = subparsers.add_parser("inspect")
    inspect_parser.add_argument("--artifact", type=parse_artifact, required=True)

    seal_parser = subparsers.add_parser("seal")
    seal_parser.add_argument("--scope", choices=sorted(EXPECTED_SCOPES), required=True)
    seal_parser.add_argument("--source", type=Path, required=True)
    seal_parser.add_argument("--evidence", type=Path, required=True)
    seal_parser.add_argument("--artifact", action="append", type=parse_artifact, required=True)

    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("--source", type=Path, required=True)
    verify_parser.add_argument("--evidence", type=Path, required=True)
    verify_parser.add_argument("--artifact", action="append", type=parse_artifact, required=True)

    args = parser.parse_args()
    try:
        if args.command == "verify-source":
            result = verify_source(args.source)
        elif args.command == "inspect":
            label, artifact = args.artifact
            result = inspect_artifact(label, artifact.read_bytes())
        elif args.command == "seal":
            result = seal(args)
        else:
            result = verify(args.evidence, args.source, args.artifact)
    except (EvidenceError, OSError, KeyError, TypeError, ValueError) as exc:
        code = exc.code if isinstance(exc, EvidenceError) else "unhandled-evidence-error"
        print(f"FAIL[{code}]: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
