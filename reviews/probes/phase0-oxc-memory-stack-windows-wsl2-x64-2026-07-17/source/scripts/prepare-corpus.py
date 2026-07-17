#!/usr/bin/env python3
"""Export the exact named JavaScript/TypeScript corpus without reading worktree bytes."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path, PurePosixPath


SCHEMA = "lumin-phase0-oxc-corpus-v1"
CORPUS_ID = "lumin-lab-35290cb-plus-stack-stress-v1"
SOURCE_REPOSITORY = "https://github.com/annyeong844/lumin_lab.git"
SOURCE_COMMIT = "35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0"
SOURCE_EXTENSIONS = {".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"}
EXPECTED_LEGACY_FILES = 705
EXPECTED_LEGACY_BYTES = 7_302_528


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    return parser.parse_args()


def git(repo: Path, *arguments: str) -> bytes:
    completed = subprocess.run(
        ["git", "-C", str(repo), *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(f"git {' '.join(arguments)} failed: {detail}")
    return completed.stdout


def validate_path(raw: str) -> PurePosixPath:
    if not raw or "\\" in raw or raw.startswith("/"):
        raise RuntimeError(f"noncanonical archive path: {raw!r}")
    path = PurePosixPath(raw)
    if path.as_posix() != raw or any(part in {"", ".", ".."} for part in path.parts):
        raise RuntimeError(f"noncanonical archive path: {raw!r}")
    return path


def exact_source_objects(repo: Path) -> list[tuple[PurePosixPath, bytes, int]]:
    tree = git(repo, "ls-tree", "-r", "-l", "-z", SOURCE_COMMIT)
    objects: list[tuple[PurePosixPath, bytes, int]] = []
    for record in tree.split(b"\0"):
        if not record:
            continue
        header, raw_path = record.split(b"\t", 1)
        mode, kind, object_id, raw_size = header.split()
        try:
            relative = validate_path(raw_path.decode("utf-8"))
        except UnicodeDecodeError as error:
            raise RuntimeError("named corpus contains a non-UTF-8 Git path") from error
        if relative.suffix.lower() not in SOURCE_EXTENSIONS:
            continue
        if kind != b"blob" or mode not in {b"100644", b"100755"}:
            raise RuntimeError(f"supported source is not a regular Git blob: {relative.as_posix()}")
        objects.append((relative, object_id, int(raw_size)))
    return objects


def read_exact_blobs(
    repo: Path, objects: list[tuple[PurePosixPath, bytes, int]]
) -> list[tuple[PurePosixPath, bytes]]:
    completed = subprocess.run(
        ["git", "-C", str(repo), "cat-file", "--batch"],
        input=b"".join(object_id + b"\n" for _, object_id, _ in objects),
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(f"git cat-file --batch failed: {detail}")
    blobs: list[tuple[PurePosixPath, bytes]] = []
    response = completed.stdout
    offset = 0
    for relative, expected_id, expected_size in objects:
        header_end = response.find(b"\n", offset)
        if header_end < 0:
            raise RuntimeError(f"missing git cat-file header for {relative.as_posix()}")
        header = response[offset:header_end]
        offset = header_end + 1
        fields = header.split()
        if len(fields) != 3:
            raise RuntimeError(f"invalid git cat-file response for {relative.as_posix()}: {header!r}")
        object_id, kind, raw_size = fields
        if object_id != expected_id or kind != b"blob" or int(raw_size) != expected_size:
            raise RuntimeError(f"git object identity changed for {relative.as_posix()}")
        data = response[offset : offset + expected_size]
        offset += expected_size
        if len(data) != expected_size or response[offset : offset + 1] != b"\n":
            raise RuntimeError(f"short git object read for {relative.as_posix()}")
        offset += 1
        blobs.append((relative, data))
    if offset != len(response):
        raise RuntimeError("unexpected trailing git cat-file response bytes")
    return blobs


def ensure_new_target(output: Path, manifest: Path) -> None:
    if output.exists():
        if not output.is_dir() or any(output.iterdir()):
            raise RuntimeError(f"output must be absent or empty: {output}")
    else:
        output.mkdir(parents=True)
    if manifest.exists():
        raise RuntimeError(f"manifest already exists: {manifest}")
    manifest.parent.mkdir(parents=True, exist_ok=True)


def write_source(output: Path, relative: PurePosixPath, data: bytes) -> None:
    target = output.joinpath(*relative.parts)
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists():
        raise RuntimeError(f"duplicate corpus path: {relative.as_posix()}")
    target.write_bytes(data)


def stress_sources() -> dict[str, bytes]:
    nested_js = b"export const nested = " + (b"(" * 512) + b"1" + (b")" * 512) + b";\n"
    nested_ts = (
        b"export type NestedObject = "
        + (b"{ value: " * 256)
        + b"number"
        + (b"; }" * 256)
        + b";\n"
    )
    nested_tsx = (
        b"export const NestedTree = ("
        + (b"<Node>" * 256)
        + b"leaf"
        + (b"</Node>" * 256)
        + b");\n"
    )
    declarations = b"".join(
        f"export const value_{index:04d}: number = {index};\n".encode("ascii")
        for index in range(4096)
    )
    return {
        "__lumin_stress__/nested-parentheses-512.js": nested_js,
        "__lumin_stress__/nested-object-types-256.ts": nested_ts,
        "__lumin_stress__/nested-tsx-elements-256.tsx": nested_tsx,
        "__lumin_stress__/top-level-declarations-4096.ts": declarations,
    }


def main() -> None:
    args = parse_args()
    repo = args.repo.resolve()
    output = args.output.resolve()
    manifest_path = args.manifest.resolve()
    ensure_new_target(output, manifest_path)

    resolved = git(repo, "rev-parse", f"{SOURCE_COMMIT}^{{commit}}").decode("ascii").strip()
    if resolved != SOURCE_COMMIT:
        raise RuntimeError(f"unexpected source commit: {resolved}")
    entries: list[dict[str, object]] = []
    legacy_bytes = 0
    objects = exact_source_objects(repo)
    for relative, data in read_exact_blobs(repo, objects):
        write_source(output, relative, data)
        entries.append(
            {
                "path": relative.as_posix(),
                "class": "legacy-exact-git",
                "bytes": len(data),
                "sha256": sha256(data),
            }
        )
        legacy_bytes += len(data)

    if len(entries) != EXPECTED_LEGACY_FILES or legacy_bytes != EXPECTED_LEGACY_BYTES:
        raise RuntimeError(
            "named corpus identity mismatch: "
            f"observed {len(entries)} files/{legacy_bytes} bytes, expected "
            f"{EXPECTED_LEGACY_FILES}/{EXPECTED_LEGACY_BYTES}"
        )

    synthetic_bytes = 0
    for raw_path, data in stress_sources().items():
        relative = validate_path(raw_path)
        write_source(output, relative, data)
        entries.append(
            {
                "path": relative.as_posix(),
                "class": "synthetic-stack",
                "bytes": len(data),
                "sha256": sha256(data),
            }
        )
        synthetic_bytes += len(data)

    entries.sort(key=lambda entry: str(entry["path"]).encode("utf-8"))
    if len({entry["path"] for entry in entries}) != len(entries):
        raise RuntimeError("duplicate corpus paths")
    manifest = {
        "schema": SCHEMA,
        "corpus_id": CORPUS_ID,
        "source_repository": SOURCE_REPOSITORY,
        "source_commit": SOURCE_COMMIT,
        "generator_sha256": sha256(Path(__file__).read_bytes()),
        "legacy_file_count": EXPECTED_LEGACY_FILES,
        "legacy_bytes": legacy_bytes,
        "synthetic_file_count": len(entries) - EXPECTED_LEGACY_FILES,
        "synthetic_bytes": synthetic_bytes,
        "entries": entries,
    }
    encoded = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    manifest_path.write_bytes(encoded)
    print(
        json.dumps(
            {
                "status": "PASS",
                "files": len(entries),
                "bytes": legacy_bytes + synthetic_bytes,
                "manifest_sha256": sha256(encoded),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
