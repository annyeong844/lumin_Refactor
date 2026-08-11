"""Authenticate Cargo registry archives and extracted source trees."""

from __future__ import annotations

import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
from collections.abc import Mapping, Sequence


MINIMUM_PYTHON = (3, 11)
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
COMPONENT = re.compile(r"^[A-Za-z0-9._+-]+$")
CHECKSUM = re.compile(r"^[0-9a-f]{64}$")
DEVICE_NAMES = frozenset(
    {"con", "prn", "aux", "nul"}
    | {f"com{index}" for index in range(1, 10)}
    | {f"lpt{index}" for index in range(1, 10)}
)
PACKAGE_FIELDS = frozenset(
    {"name", "version", "source", "checksum", "manifestPath"}
)
ROOT_FIELDS = frozenset(
    {"schemaVersion", "repositoryRoot", "cargoHome", "packages"}
)


class SnapshotError(RuntimeError):
    """Registry bytes cannot authorize compilation."""


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            raise SnapshotError(f"duplicate JSON key: {key!r}")
        value[key] = item
    return value


def _object(value: object, fields: frozenset[str], context: str) -> Mapping[str, object]:
    if not isinstance(value, dict):
        raise SnapshotError(f"{context} must be an object")
    if set(value) != fields:
        raise SnapshotError(
            f"{context} fields differ: expected {sorted(fields)!r}, got {sorted(value)!r}"
        )
    return value


def _string(value: object, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise SnapshotError(f"{context} must be a non-empty string")
    return value


def _absolute(path: Path) -> Path:
    return Path(os.path.abspath(path))


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def _require_unredirected(path: Path, kind: str, context: str) -> Path:
    lexical = _absolute(path)
    try:
        physical = lexical.resolve(strict=True)
    except OSError as error:
        raise SnapshotError(f"cannot resolve {context} {lexical}: {error}") from error
    if physical != lexical or lexical.is_symlink():
        raise SnapshotError(f"redirected {context}: {lexical} -> {physical}")
    if kind == "file" and not lexical.is_file():
        raise SnapshotError(f"{context} is not a regular file: {lexical}")
    if kind == "directory" and not lexical.is_dir():
        raise SnapshotError(f"{context} is not a directory: {lexical}")
    return lexical


def ensure_runtime(repository_root: Path) -> None:
    if sys.version_info < MINIMUM_PYTHON:
        raise SnapshotError("Python 3.11 or newer is required")
    if sys.flags.isolated != 1 or sys.flags.no_site != 1 or not sys.flags.safe_path:
        raise SnapshotError("invoke registry_snapshot.py with Python -I -S")
    for entry in sys.path:
        if not entry:
            raise SnapshotError("empty Python import path is forbidden")
        if _is_within(Path(entry).resolve(strict=False), repository_root):
            raise SnapshotError(f"repository Python import path is forbidden: {entry}")


def _ascii_field(raw: bytes, context: str, *, allow_empty: bool) -> str:
    nul = raw.find(b"\0")
    if nul < 0:
        payload = raw
    else:
        payload = raw[:nul]
        if any(raw[nul:]):
            raise SnapshotError(f"{context} has non-NUL bytes after padding")
    if not payload and not allow_empty:
        raise SnapshotError(f"{context} is empty")
    try:
        return payload.decode("ascii")
    except UnicodeDecodeError as error:
        raise SnapshotError(f"{context} is not ASCII") from error


def _octal(raw: bytes, context: str) -> int:
    stripped = raw.strip(b"\0 ")
    if not stripped or any(byte < ord("0") or byte > ord("7") for byte in stripped):
        raise SnapshotError(f"{context} is not NUL/space-padded ASCII octal")
    first = raw.find(stripped)
    if first < 0 or any(byte not in b"\0 " for byte in raw[:first] + raw[first + len(stripped) :]):
        raise SnapshotError(f"{context} has noncanonical octal padding")
    return int(stripped, 8)


def _validate_component(component: str, context: str) -> None:
    if (
        not COMPONENT.fullmatch(component)
        or component in {".", ".."}
        or component.endswith(".")
    ):
        raise SnapshotError(f"{context} has a nonportable component: {component!r}")
    stem = component.split(".", 1)[0].lower()
    if stem in DEVICE_NAMES:
        raise SnapshotError(f"{context} uses a Windows device component: {component!r}")


def _member_identity(header: bytes, package_prefix: str) -> tuple[str, str]:
    name = _ascii_field(header[0:100], "USTAR name", allow_empty=False)
    prefix = _ascii_field(header[345:500], "USTAR prefix", allow_empty=True)
    authored = f"{prefix}/{name}" if prefix else name
    components = authored.split("/")
    if any(not component for component in components):
        raise SnapshotError(f"USTAR member has an empty path component: {authored!r}")
    for component in components:
        _validate_component(component, f"USTAR member {authored!r}")
    if components[0] != package_prefix or len(components) == 1:
        raise SnapshotError(
            f"USTAR member is outside exact package prefix {package_prefix!r}: {authored!r}"
        )
    relative = "/".join(components[1:])
    return relative, relative.lower()


def _read_exact(stream: gzip.GzipFile, length: int, context: str) -> bytes:
    data = stream.read(length)
    if len(data) != length:
        raise SnapshotError(f"short {context}: expected {length}, got {len(data)}")
    return data


def archive_snapshot(archive: Path, package_prefix: str) -> dict[str, tuple[int, str, bool]]:
    files: dict[str, tuple[int, str, bool]] = {}
    identities: set[str] = set()
    try:
        with archive.open("rb") as raw, gzip.GzipFile(fileobj=raw, mode="rb") as stream:
            while True:
                header = _read_exact(stream, 512, "USTAR header")
                if header == bytes(512):
                    if _read_exact(stream, 512, "second USTAR terminator") != bytes(512):
                        raise SnapshotError("USTAR archive has only one terminal zero block")
                    if stream.read(1):
                        raise SnapshotError("USTAR archive has trailing decompressed payload")
                    break
                if (header[257:263], header[263:265]) not in {
                    (b"ustar\0", b"00"),
                    (b"ustar ", b" \0"),
                }:
                    raise SnapshotError("unsupported USTAR magic/version")
                if header[156:157] != b"0":
                    raise SnapshotError(
                        f"unsupported USTAR member type: {header[156:157]!r}"
                    )
                expected_checksum = _octal(header[148:156], "USTAR checksum")
                checksum_header = bytearray(header)
                checksum_header[148:156] = b" " * 8
                if sum(checksum_header) != expected_checksum:
                    raise SnapshotError("USTAR header checksum mismatch")
                mode = _octal(header[100:108], "USTAR mode")
                size = _octal(header[124:136], "USTAR size")
                relative, identity = _member_identity(header, package_prefix)
                if identity in identities:
                    raise SnapshotError(f"duplicate/case-colliding USTAR member: {relative}")
                identities.add(identity)
                content = _read_exact(stream, size, f"USTAR member {relative}")
                padding = (-size) % 512
                if padding and _read_exact(stream, padding, f"USTAR padding for {relative}") != bytes(padding):
                    raise SnapshotError(f"USTAR member has nonzero padding: {relative}")
                files[relative] = (
                    size,
                    hashlib.sha256(content).hexdigest(),
                    bool(mode & 0o111),
                )
    except (OSError, EOFError, gzip.BadGzipFile) as error:
        raise SnapshotError(f"cannot read authenticated archive {archive}: {error}") from error
    if not files:
        raise SnapshotError(f"USTAR archive contains zero regular files: {archive}")
    return files


def _expected_directories(files: Mapping[str, object]) -> set[str]:
    directories: set[str] = set()
    for relative in files:
        parts = relative.split("/")[:-1]
        for depth in range(1, len(parts) + 1):
            directories.add("/".join(parts[:depth]))
    return directories


def validate_extracted_tree(
    package_root: Path,
    expected: Mapping[str, tuple[int, str, bool]],
) -> None:
    expected_directories = _expected_directories(expected)
    observed_files: set[str] = set()
    observed_directories: set[str] = set()
    identities: set[str] = set()
    inodes: set[tuple[int, int]] = set()
    pending = [package_root]
    while pending:
        directory = pending.pop()
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as error:
            raise SnapshotError(f"cannot enumerate registry tree {directory}: {error}") from error
        for entry in entries:
            path = Path(entry.path)
            relative = path.relative_to(package_root).as_posix()
            _validate_component(entry.name, f"registry tree {relative!r}")
            identity = relative.lower()
            if identity in identities:
                raise SnapshotError(f"case-colliding registry tree path: {relative}")
            identities.add(identity)
            lexical = _absolute(path)
            try:
                physical = lexical.resolve(strict=True)
                metadata = os.stat(path, follow_symlinks=False)
            except OSError as error:
                raise SnapshotError(f"cannot inspect registry tree path {path}: {error}") from error
            if entry.is_symlink() or physical != lexical:
                raise SnapshotError(f"redirected registry tree path: {lexical} -> {physical}")
            if stat.S_ISDIR(metadata.st_mode):
                observed_directories.add(relative)
                pending.append(path)
                continue
            if not stat.S_ISREG(metadata.st_mode):
                raise SnapshotError(f"unsupported registry tree node: {relative}")
            inode = (metadata.st_dev, metadata.st_ino)
            if metadata.st_nlink != 1 or inode in inodes:
                raise SnapshotError(f"hard-linked registry tree path: {relative}")
            inodes.add(inode)
            if relative == ".cargo-ok":
                continue
            wanted = expected.get(relative)
            if wanted is None:
                raise SnapshotError(f"extra registry source file: {relative}")
            try:
                content = path.read_bytes()
            except OSError as error:
                raise SnapshotError(f"cannot read registry source file {path}: {error}") from error
            size, digest, executable = wanted
            observed_executable = bool(metadata.st_mode & 0o111)
            if len(content) != size or hashlib.sha256(content).hexdigest() != digest:
                raise SnapshotError(f"registry source content differs from archive: {relative}")
            if os.name != "nt" and observed_executable != executable:
                raise SnapshotError(f"registry executable bit differs from archive: {relative}")
            observed_files.add(relative)
    missing_files = sorted(set(expected) - observed_files)
    if missing_files:
        raise SnapshotError(f"missing registry source files: {missing_files!r}")
    if observed_directories != expected_directories:
        raise SnapshotError(
            "registry source directory surface differs: "
            f"expected {sorted(expected_directories)!r}, got {sorted(observed_directories)!r}"
        )


def validate_package(
    repository_root: Path,
    cargo_home: Path,
    raw_package: object,
) -> tuple[str, str]:
    package = _object(raw_package, PACKAGE_FIELDS, "registry package")
    name = _string(package["name"], "registry package name")
    version = _string(package["version"], "registry package version")
    _validate_component(name, "registry package name")
    _validate_component(version, "registry package version")
    if package["source"] != CRATES_IO_SOURCE:
        raise SnapshotError(f"registry package has unapproved source: {name} {version}")
    checksum = _string(package["checksum"], f"checksum for {name} {version}")
    if not CHECKSUM.fullmatch(checksum):
        raise SnapshotError(f"invalid SHA-256 checksum for {name} {version}: {checksum!r}")
    manifest = _require_unredirected(
        Path(_string(package["manifestPath"], f"manifest for {name} {version}")),
        "file",
        f"registry manifest for {name} {version}",
    )
    package_root = _require_unredirected(
        manifest.parent, "directory", f"registry package root for {name} {version}"
    )
    if manifest.name != "Cargo.toml":
        raise SnapshotError(f"registry manifest has unexpected name: {manifest}")
    registry_src = _require_unredirected(
        cargo_home / "registry" / "src", "directory", "Cargo registry source root"
    )
    try:
        relative_root = package_root.relative_to(registry_src)
    except ValueError as error:
        raise SnapshotError(f"registry package escapes Cargo home: {package_root}") from error
    if len(relative_root.parts) != 2 or relative_root.parts[1] != f"{name}-{version}":
        raise SnapshotError(f"registry package path has unexpected identity: {relative_root}")
    registry_key = relative_root.parts[0]
    _validate_component(registry_key, "Cargo registry cache key")
    archive = _require_unredirected(
        cargo_home / "registry" / "cache" / registry_key / f"{name}-{version}.crate",
        "file",
        f"registry archive for {name} {version}",
    )
    if _is_within(package_root, repository_root) or _is_within(archive, repository_root):
        raise SnapshotError(f"registry package is repository-owned: {name} {version}")
    try:
        archive_digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    except OSError as error:
        raise SnapshotError(f"cannot hash registry archive {archive}: {error}") from error
    if archive_digest != checksum:
        raise SnapshotError(
            f"registry archive checksum mismatch for {name} {version}: "
            f"expected {checksum}, got {archive_digest}"
        )
    expected = archive_snapshot(archive, f"{name}-{version}")
    validate_extracted_tree(package_root, expected)
    return name, version


def validate_snapshot(envelope: object) -> dict[str, object]:
    root = _object(envelope, ROOT_FIELDS, "registry snapshot envelope")
    if root["schemaVersion"] != 1:
        raise SnapshotError("registry snapshot schemaVersion must be 1")
    repository_root = _require_unredirected(
        Path(_string(root["repositoryRoot"], "repository root")),
        "directory",
        "repository root",
    )
    ensure_runtime(repository_root)
    cargo_home = _require_unredirected(
        Path(_string(root["cargoHome"], "Cargo home")), "directory", "Cargo home"
    )
    if _is_within(cargo_home, repository_root):
        raise SnapshotError(f"Cargo home is repository-owned: {cargo_home}")
    packages = root["packages"]
    if not isinstance(packages, list) or not packages:
        raise SnapshotError("registry snapshot packages must be a non-empty array")
    observed: list[tuple[str, str]] = []
    for package in packages:
        observed.append(validate_package(repository_root, cargo_home, package))
    if observed != sorted(set(observed)) or len(observed) != len(set(observed)):
        raise SnapshotError("registry packages are duplicate or not deterministically sorted")
    return {"schemaVersion": 1, "packageCount": len(observed)}


def _read_stdin() -> object:
    try:
        return json.loads(sys.stdin.read(), object_pairs_hook=_unique_object)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise SnapshotError(f"cannot parse registry snapshot envelope: {error}") from error


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        if tuple(sys.argv[1:] if arguments is None else arguments):
            raise SnapshotError("registry_snapshot.py accepts only a stdin envelope")
        verdict = validate_snapshot(_read_stdin())
        print(json.dumps(verdict, sort_keys=True, separators=(",", ":")))
        return 0
    except SnapshotError as error:
        print(f"[registry-snapshot] {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
