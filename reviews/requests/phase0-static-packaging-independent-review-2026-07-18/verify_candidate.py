#!/usr/bin/env python3
"""Author-side consistency verifier for the static-packaging review candidate."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import struct
import subprocess
import sys
from typing import Any
import zipfile


CANDIDATE = "e0a2810b46f6091895b5e9f7dd4454e8854fee0e"
PARENT = "47f8fa9693bb8ecae2cff3ff72e54b0d259676d0"
GATE_BASE = "84fe32d2e57ec10399964999a4a5a60563944a2b"
SOURCE_COMMIT = "e9f4c4f692b8027063ae1ad0909fe97bebc5dc30"
EVIDENCE_COMMIT = "47f8fa9693bb8ecae2cff3ff72e54b0d259676d0"
SUBJECT = "Normalize static packaging packet manifest"
TREE = "bed2f4dde295bcb0b0b9eaf78d20877468d236fd"
PACKET_PREFIX = "reviews/probes/phase0-static-packaging-windows-wsl2-native-linux-x64-2026-07-18"
PACKET_MANIFEST_SHA256 = "b1c685a535a5c5e36011de7b59fd89d8400a29d0b5d06db874b629c6e8180ed7"
SOURCE_MANIFEST_SHA256 = "dd30eeda67caf9e354838a9ec7974cdd3dc118a9136c2556fcfe56c9f441db45"
ARCHITECTURE_MANIFEST_SHA256 = "e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a"
NATIVE_ARCHIVE_SHA256 = "073ef5907944f8b79df8eab07d135826365f143c4d590ee3d59d7f57d5926454"
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
    "windows-ntfs": (11, "0058fc941ced83f449d4cb04f225a46eadd4b977e3bc1bb4a273693c6e936117", {"windows-msvc"}),
    "wsl2-ext4": (18, "adbf074916222c1590319fca65318c27dbed25d2913173d0a34891a071b55bf2", {"linux-gnu", "linux-musl"}),
    "native-linux-ext4": (18, "3f43d654d612c222c8ebb4be1f4ece1d36066cc17b8354dbee55ba4d1dad2523", {"linux-gnu", "linux-musl"}),
}
ARTIFACTS = {
    "windows-msvc": ("8fb4a84858e6e0118d29ab4c53de5b7d8f3275df1146dd6bdbbaafe57960051b", 1411584),
    "wsl2-gnu": ("bf558042793a029807db457821ac8030843bbc2494409b46198564a9c8a3bd22", 1795120),
    "wsl2-musl": ("7d810082567f4b437a5d7aaef6db9a3677fd21d9ef71cc1e659c56679aad04a9", 1897184),
    "native-gnu": ("790a5afb5c73bb03098939c8bc00a1751398f66774367da2f1b16b423cd9b8c2", 1795184),
    "native-musl": ("8704f655bd0f53456d7da3dae3dbecd13c7223083da5d8f516d8aca5046bb33e", 1897184),
}
REQUEST_DIR = Path(__file__).resolve().parent


class VerificationError(RuntimeError):
    pass


def run_git(repo: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise VerificationError(
            f"git {' '.join(args)} failed: {result.stderr.decode(errors='replace').strip()}"
        )
    return result.stdout


def git_text(repo: Path, *args: str) -> str:
    return run_git(repo, *args).decode("utf-8").strip()


def git_blob(repo: Path, revision: str, path: str) -> bytes:
    return run_git(repo, "cat-file", "blob", f"{revision}:{path}")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


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
    manifest_path: str,
    expected_sha: str,
    count: int,
) -> list[tuple[str, str]]:
    data = git_blob(repo, CANDIDATE, manifest_path)
    if sha256(data) != expected_sha:
        raise VerificationError(f"manifest digest mismatch: {manifest_path}")
    entries = parse_manifest(data, manifest_path, count)
    for expected, relative in entries:
        actual = git_blob(repo, CANDIDATE, f"{base}/{relative}")
        if sha256(actual) != expected:
            raise VerificationError(f"manifest member mismatch: {base}/{relative}")
    tree_paths = {
        path
        for path in git_text(repo, "ls-tree", "-r", "--name-only", CANDIDATE, base).splitlines()
        if path
    }
    expected_paths = {f"{base}/{relative}" for _, relative in entries} | {manifest_path}
    if tree_paths != expected_paths:
        raise VerificationError(f"manifest inventory mismatch under {base}")
    return entries


def root_dependencies(metadata: dict[str, Any]) -> dict[str, str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    root = metadata["resolve"]["root"]
    node = next(item for item in metadata["resolve"]["nodes"] if item["id"] == root)
    return {
        packages[dependency["pkg"]]["name"]: packages[dependency["pkg"]]["version"]
        for dependency in node["deps"]
    }


def elf64_x86(data: bytes) -> bool:
    return (
        len(data) >= 20
        and data[:6] == b"\x7fELF\x02\x01"
        and struct.unpack_from("<H", data, 18)[0] == 62
    )


def pe32_x86(data: bytes) -> bool:
    if len(data) < 64 or data[:2] != b"MZ":
        return False
    offset = struct.unpack_from("<I", data, 0x3C)[0]
    return (
        offset + 26 <= len(data)
        and data[offset : offset + 4] == b"PE\0\0"
        and struct.unpack_from("<H", data, offset + 4)[0] == 0x8664
        and struct.unpack_from("<H", data, offset + 24)[0] == 0x20B
    )


def verify(repo: Path, artifact_root: Path | None) -> dict[str, Any]:
    results: list[dict[str, str]] = []

    def check(name: str, condition: bool, evidence: str) -> None:
        if not condition:
            raise VerificationError(f"{name}: {evidence}")
        results.append({"name": name, "status": "PASS", "evidence": evidence})

    check("candidate-object", git_text(repo, "cat-file", "-t", CANDIDATE) == "commit", CANDIDATE)
    metadata = git_text(repo, "show", "-s", "--format=%P%n%s%n%T", CANDIDATE).splitlines()
    check("candidate-parent", metadata[0] == PARENT, metadata[0])
    check("candidate-subject", metadata[1] == SUBJECT, metadata[1])
    check("candidate-tree", metadata[2] == TREE, metadata[2])
    commits = git_text(repo, "rev-list", "--reverse", f"{GATE_BASE}..{CANDIDATE}").splitlines()
    check(
        "gate-commit-range",
        commits == [SOURCE_COMMIT, EVIDENCE_COMMIT, CANDIDATE],
        repr(commits),
    )
    changed = git_text(repo, "diff", "--name-only", GATE_BASE, CANDIDATE).splitlines()
    check(
        "gate-path-scope",
        bool(changed) and all(path.startswith(f"{PACKET_PREFIX}/") for path in changed),
        f"{len(changed)} paths under probe packet only",
    )
    check("no-product-workflow", not git_text(repo, "ls-tree", "-r", "--name-only", CANDIDATE, ".github"), "no .github tree in candidate")

    packet_entries = verify_git_manifest(
        repo, PACKET_PREFIX, f"{PACKET_PREFIX}/SHA256SUMS", PACKET_MANIFEST_SHA256, 65
    )
    check("packet-manifest", len(packet_entries) == 65, "65/65 exact Git blobs")
    source_entries = verify_git_manifest(
        repo,
        f"{PACKET_PREFIX}/source",
        f"{PACKET_PREFIX}/source/SHA256SUMS",
        SOURCE_MANIFEST_SHA256,
        9,
    )
    check("source-manifest", len(source_entries) == 9, "9/9 exact source blobs")

    architecture_manifest = (REQUEST_DIR / "architecture-manifest.txt").read_bytes()
    check("architecture-manifest-digest", sha256(architecture_manifest) == ARCHITECTURE_MANIFEST_SHA256, sha256(architecture_manifest))
    architecture_entries = parse_manifest(architecture_manifest, "architecture-manifest.txt", 16)
    for expected, path in architecture_entries:
        if sha256(git_blob(repo, CANDIDATE, path)) != expected:
            raise VerificationError(f"architecture blob mismatch: {path}")
    check("architecture-binding", True, "16/16 exact candidate blobs")

    summaries: dict[str, dict[str, Any]] = {}
    for scope, (count, digest_value, expected_labels) in SCOPES.items():
        base = f"{PACKET_PREFIX}/evidence/{scope}"
        entries = verify_git_manifest(
            repo, base, f"{base}/SHA256SUMS", digest_value, count
        )
        check(f"evidence-manifest:{scope}", len(entries) == count, f"{count}/{count}")
        summary = strict_json(git_blob(repo, CANDIDATE, f"{base}/summary.json"), scope)
        summaries[scope] = summary
        labels = {item["label"] for item in summary["artifacts"]}
        check(
            f"summary:{scope}",
            summary["status"] == "PASS"
            and summary["sourceManifestSha256"] == SOURCE_MANIFEST_SHA256
            and labels == expected_labels
            and summary["claimBoundary"]["staticPackagingFeasibilityOnly"] is True,
            repr(labels),
        )
        cargo_metadata = strict_json(
            git_blob(repo, CANDIDATE, f"{base}/cargo-metadata.json"), f"{scope}/metadata"
        )
        links = sorted(
            f"{package['name']}@{package['version']}:{package['links']}"
            for package in cargo_metadata["packages"]
            if package.get("links")
        )
        check(f"dependencies:{scope}", root_dependencies(cargo_metadata) == EXPECTED_DEPENDENCIES, "exact direct versions")
        check(f"cargo-links:{scope}", links == ["rayon-core@1.13.0:rayon-core"], repr(links))
        for label in expected_labels:
            run = strict_json(git_blob(repo, CANDIDATE, f"{base}/run-{label}.json"), f"{scope}/{label}")
            check(
                f"run-oracle:{scope}/{label}",
                run["status"] == "PASS"
                and run["oxcStatementCount"] == 2
                and run["rayonSum"] == 4950
                and run["redbValue"] == 42,
                repr(run),
            )
        if "linux-musl" in expected_labels:
            linkage = git_blob(repo, CANDIDATE, f"{base}/linkage-linux-musl.txt").decode("utf-8")
            check(
                f"static-musl:{scope}",
                "INTERP" not in linkage
                and "Requesting program interpreter" not in linkage
                and "(NEEDED)" not in linkage
                and ("statically linked" in linkage or "not a dynamic executable" in linkage),
                "no interpreter or NEEDED",
            )

    native_checks = strict_json(
        git_blob(repo, CANDIDATE, f"{PACKET_PREFIX}/runner/native-independent-checks.json"),
        "native-independent-checks",
    )
    check(
        "native-independent-checks",
        native_checks["status"] == "PASS"
        and native_checks["checkCount"] == 61
        and native_checks["runnerCommit"] == "721984d52e75d2385948767ce8ade6f190babaf2",
        "61/61 author-side artifact checks",
    )

    if artifact_root is not None:
        root = artifact_root.resolve(strict=True)
        detached = {
            "windows-msvc": (root / "artifacts/windows-msvc/lumin-phase0-static-packaging-probe.exe").read_bytes(),
            "wsl2-gnu": (root / "artifacts/wsl2-gnu/lumin-phase0-static-packaging-probe").read_bytes(),
            "wsl2-musl": (root / "artifacts/wsl2-musl/lumin-phase0-static-packaging-probe").read_bytes(),
        }
        native_zip = root / "artifacts/native-workflow.zip"
        check("native-archive", sha256(native_zip.read_bytes()) == NATIVE_ARCHIVE_SHA256, sha256(native_zip.read_bytes()))
        with zipfile.ZipFile(native_zip) as archive:
            detached["native-gnu"] = archive.read(
                "source/target/x86_64-unknown-linux-gnu/release/lumin-phase0-static-packaging-probe"
            )
            detached["native-musl"] = archive.read(
                "source/target/x86_64-unknown-linux-musl/release/lumin-phase0-static-packaging-probe"
            )
        for label, data in detached.items():
            expected_hash, expected_size = ARTIFACTS[label]
            check(f"artifact-identity:{label}", sha256(data) == expected_hash and len(data) == expected_size, f"{len(data)} {sha256(data)}")
            format_ok = pe32_x86(data) if label == "windows-msvc" else elf64_x86(data)
            check(f"artifact-format:{label}", format_ok, "PE32+ or ELF64 x86-64")

    request = strict_json(
        (REQUEST_DIR / "static-packaging-request.json").read_bytes(), "request"
    )
    check(
        "request-binding",
        request["candidate"] == CANDIDATE
        and request["packet_manifest"]["sha256"] == PACKET_MANIFEST_SHA256
        and request["source_manifest"]["sha256"] == SOURCE_MANIFEST_SHA256,
        "request exact identities",
    )
    return {
        "schema": "lumin-phase0-static-packaging-author-preflight-v1",
        "status": "PASS",
        "candidate": CANDIDATE,
        "packetManifestSha256": PACKET_MANIFEST_SHA256,
        "sourceManifestSha256": SOURCE_MANIFEST_SHA256,
        "checks": {"pass": len(results), "fail": 0},
        "results": results,
        "independenceBoundary": "Author-side consistency output only; not independent review evidence.",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--artifact-root", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        result = verify(args.repo.resolve(), args.artifact_root)
    except (OSError, VerificationError, ValueError, KeyError, zipfile.BadZipFile) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        return 1
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8", newline="\n")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
