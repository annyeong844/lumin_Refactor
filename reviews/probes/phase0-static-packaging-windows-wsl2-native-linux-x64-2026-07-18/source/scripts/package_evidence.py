#!/usr/bin/env python3
"""Seal and verify standalone Phase 0 static-packaging evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import struct
import sys
from typing import Any


ARCHITECTURE_CANDIDATE = "9a0dbe5c89463892c001e864c4f18eeab9e0eaed"
ARCHITECTURE_MANIFEST = "e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a"
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
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def strict_json(path: Path) -> Any:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise EvidenceError(f"duplicate JSON key in {path}: {key}")
            result[key] = value
        return result

    try:
        return json.loads(
            path.read_text(encoding="utf-8"), object_pairs_hook=reject_duplicates
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"invalid JSON {path}: {exc}") from exc


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
        if source and any(part == "target" or part.startswith("target-") for part in relative.parts):
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


def artifact_format(label: str, data: bytes) -> str:
    if label == "windows-msvc":
        if len(data) < 64 or data[:2] != b"MZ":
            raise EvidenceError("Windows artifact is not an MZ executable")
        pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
        if pe_offset + 26 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
            raise EvidenceError("Windows artifact has no PE signature")
        machine = struct.unpack_from("<H", data, pe_offset + 4)[0]
        optional_magic = struct.unpack_from("<H", data, pe_offset + 24)[0]
        if machine != 0x8664 or optional_magic != 0x20B:
            raise EvidenceError(
                f"Windows artifact is not PE32+ x86-64: machine={machine:#x} "
                f"magic={optional_magic:#x}"
            )
        return "PE32+-x86_64"
    if (
        len(data) < 20
        or data[:4] != b"\x7fELF"
        or data[4] != 2
        or data[5] != 1
    ):
        raise EvidenceError(f"{label} artifact is not little-endian ELF64")
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != 62:
        raise EvidenceError(f"{label} artifact machine is not x86-64: {machine}")
    return "ELF64-x86_64"


def validate_run(path: Path, label: str) -> dict[str, Any]:
    run = strict_json(path)
    expected_os, expected_env = EXPECTED_RUNS[label]
    expected = {
        "schema": "lumin-phase0-static-packaging-run-v1",
        "status": "PASS",
        "os": expected_os,
        "arch": "x86_64",
        "targetEnv": expected_env,
        "oxcStatementCount": 2,
        "rayonSum": 4950,
        "redbValue": 42,
    }
    if run != expected:
        raise EvidenceError(f"unexpected run result for {label}: {run!r}")
    stderr = path.with_name(f"{path.stem}.stderr.log")
    if not stderr.is_file() or stderr.stat().st_size != 0:
        raise EvidenceError(f"probe stderr is missing or nonempty: {stderr}")
    return run


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


def validate_host(host: dict[str, Any], scope: str) -> None:
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
    if not host["rustcVersion"].startswith("rustc 1.96.0 "):
        raise EvidenceError(f"wrong rustc version: {host['rustcVersion']}")
    if not host["cargoVersion"].startswith("cargo 1.96.0 "):
        raise EvidenceError(f"wrong cargo version: {host['cargoVersion']}")


def validate_linkage(path: Path, label: str) -> None:
    if not path.is_file() or path.stat().st_size == 0:
        raise EvidenceError(f"linkage evidence missing or empty: {path}")
    text = path.read_text(encoding="utf-8", errors="replace")
    if label == "linux-musl":
        if "INTERP" in text or "Requesting program interpreter" in text or "(NEEDED)" in text:
            raise EvidenceError("musl artifact contains a dynamic interpreter or dependency")
        if "not a dynamic executable" not in text and "statically linked" not in text:
            raise EvidenceError("musl artifact is not proven static")


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
        validate_run(evidence / f"run-{label}.json", label)
        validate_linkage(evidence / f"linkage-{label}.txt", label)


def write_json(path: Path, value: Any) -> None:
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def expected_artifact_format(label: str) -> str:
    return "PE32+-x86_64" if label == "windows-msvc" else "ELF64-x86_64"


def seal(args: argparse.Namespace) -> dict[str, Any]:
    source = args.source.resolve(strict=True)
    evidence = args.evidence.resolve(strict=True)
    source_identity = verify_source(source)
    spec = EXPECTED_SCOPES.get(args.scope)
    if spec is None:
        raise EvidenceError(f"unknown scope: {args.scope}")
    artifacts_by_label = dict(args.artifact)
    if len(artifacts_by_label) != len(args.artifact) or set(artifacts_by_label) != spec["labels"]:
        raise EvidenceError(
            f"artifact labels for {args.scope} must be exactly {sorted(spec['labels'])!r}"
        )

    metadata = strict_json(evidence / "cargo-metadata.json")
    if not isinstance(metadata, dict):
        raise EvidenceError("Cargo metadata must be an object")
    dependencies = root_dependency_versions(metadata)
    linked = cargo_link_declarations(metadata)
    host = strict_json(evidence / "host.json")
    if not isinstance(host, dict):
        raise EvidenceError("host identity must be an object")
    validate_host(host, args.scope)
    validate_raw_files(evidence, set(artifacts_by_label))

    artifacts = []
    for label, binary in artifacts_by_label.items():
        if not binary.is_file():
            raise EvidenceError(f"artifact missing: {binary}")
        data = binary.read_bytes()
        artifacts.append(
            {
                "format": artifact_format(label, data),
                "label": label,
                "sha256": sha256(data),
                "sizeBytes": len(data),
                "static": label == "linux-musl",
            }
        )

    summary = {
        "architectureCandidate": ARCHITECTURE_CANDIDATE,
        "architectureManifestSha256": ARCHITECTURE_MANIFEST,
        "artifacts": sorted(artifacts, key=lambda item: item["label"]),
        "cargoLinkDeclarations": linked,
        "cargoLinkInterpretation": {
            "rayon-core@1.13.0:rayon-core": "non-native one-version uniqueness sentinel"
        },
        "claimBoundary": CLAIM_BOUNDARY,
        "directDependencies": dependencies,
        "host": host,
        "unexpectedCargoLinkDeclarations": [],
        "schema": "lumin-phase0-static-packaging-summary-v1",
        "scope": args.scope,
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
    verify(evidence, source)
    return summary


def verify(evidence: Path, source: Path) -> dict[str, Any]:
    evidence = evidence.resolve(strict=True)
    source_identity = verify_source(source)
    count = verify_manifest(evidence, evidence / "SHA256SUMS", source=False)
    summary = strict_json(evidence / "summary.json")
    if not isinstance(summary, dict):
        raise EvidenceError("summary must be an object")
    scope = summary.get("scope")
    spec = EXPECTED_SCOPES.get(scope)
    if spec is None:
        raise EvidenceError(f"summary scope is invalid: {scope!r}")
    if (
        summary.get("schema") != "lumin-phase0-static-packaging-summary-v1"
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
    ):
        raise EvidenceError("summary contract or source identity mismatch")
    host = strict_json(evidence / "host.json")
    if summary.get("host") != host or not isinstance(host, dict):
        raise EvidenceError("summary host identity mismatch")
    validate_host(host, scope)
    metadata = strict_json(evidence / "cargo-metadata.json")
    if not isinstance(metadata, dict):
        raise EvidenceError("Cargo metadata must be an object")
    dependencies = root_dependency_versions(metadata)
    links = cargo_link_declarations(metadata)
    if summary.get("cargoLinkDeclarations") != links:
        raise EvidenceError("summary Cargo links declaration mismatch")
    if summary.get("directDependencies") != dependencies:
        raise EvidenceError("summary direct dependency mismatch")
    artifacts = summary.get("artifacts")
    if not isinstance(artifacts, list):
        raise EvidenceError("summary artifacts missing")
    artifact_labels = {item.get("label") for item in artifacts if isinstance(item, dict)}
    if len(artifacts) != len(spec["labels"]) or artifact_labels != spec["labels"]:
        raise EvidenceError("summary artifact labels mismatch")
    for artifact in artifacts:
        label = artifact["label"]
        if (
            artifact.get("format") != expected_artifact_format(label)
            or artifact.get("static") != (label == "linux-musl")
            or not isinstance(artifact.get("sizeBytes"), int)
            or artifact["sizeBytes"] <= 0
            or not isinstance(artifact.get("sha256"), str)
            or not HEX_SHA256.fullmatch(artifact["sha256"])
        ):
            raise EvidenceError(f"summary artifact identity invalid: {artifact!r}")
    validate_raw_files(evidence, set(spec["labels"]))
    return {"evidenceFiles": count, "status": "PASS", "summary": summary}


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    source_parser = subparsers.add_parser("verify-source")
    source_parser.add_argument("--source", type=Path, required=True)

    seal_parser = subparsers.add_parser("seal")
    seal_parser.add_argument("--scope", choices=sorted(EXPECTED_SCOPES), required=True)
    seal_parser.add_argument("--source", type=Path, required=True)
    seal_parser.add_argument("--evidence", type=Path, required=True)
    seal_parser.add_argument("--artifact", action="append", type=parse_artifact, required=True)

    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("--source", type=Path, required=True)
    verify_parser.add_argument("--evidence", type=Path, required=True)

    args = parser.parse_args()
    try:
        if args.command == "verify-source":
            result = verify_source(args.source)
        elif args.command == "seal":
            result = seal(args)
        else:
            result = verify(args.evidence, args.source)
    except (EvidenceError, OSError, KeyError, TypeError, ValueError) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
