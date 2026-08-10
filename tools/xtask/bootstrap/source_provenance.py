"""Fail-closed Cargo source-provenance bootstrap for repository CI."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tomllib
from collections.abc import Mapping, Sequence


MINIMUM_PYTHON = (3, 11)
CONFIG_NAMES = ("config.toml", "config")


class ProvenanceError(RuntimeError):
    """The Cargo invocation cannot produce an architecture verdict."""


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def _resolved(path: Path, *, base: Path | None = None) -> Path:
    candidate = path if path.is_absolute() else (base or Path.cwd()) / path
    return candidate.resolve(strict=False)


def repository_root() -> Path:
    root = Path(__file__).resolve().parents[3]
    if not (root / "Cargo.toml").is_file():
        raise ProvenanceError(f"repository Cargo.toml is missing under {root}")
    return root


def active_cargo_home(environment: Mapping[str, str], cwd: Path) -> Path:
    configured = environment.get("CARGO_HOME")
    if configured is not None:
        if not configured:
            raise ProvenanceError("CARGO_HOME must not be empty")
        return _resolved(Path(configured), base=cwd)
    return (Path.home() / ".cargo").resolve(strict=False)


def ensure_runtime(root: Path) -> None:
    if sys.version_info < MINIMUM_PYTHON:
        raise ProvenanceError(
            f"Python {MINIMUM_PYTHON[0]}.{MINIMUM_PYTHON[1]} or newer is required"
        )
    if sys.flags.isolated != 1 or sys.flags.no_site != 1 or not sys.flags.safe_path:
        raise ProvenanceError("invoke this guard with Python -I -S")
    for entry in sys.path:
        if not entry:
            raise ProvenanceError("empty Python import path is forbidden")
        if _is_within(_resolved(Path(entry)), root):
            raise ProvenanceError(
                f"repository-controlled Python import path is forbidden: {entry}"
            )


def _reject_path(path: Path, description: str) -> None:
    if path.exists() or path.is_symlink():
        raise ProvenanceError(f"forbidden {description}: {path}")


def reject_cargo_configuration(root: Path, cargo_home: Path) -> None:
    for current, directories, _files in os.walk(root, followlinks=False):
        directories[:] = [
            directory for directory in directories if directory not in {".git", "target"}
        ]
        if ".cargo" in directories:
            cargo_dir = Path(current) / ".cargo"
            for name in CONFIG_NAMES:
                _reject_path(cargo_dir / name, "repository Cargo configuration")
    for ancestor in (root, *root.parents):
        cargo_dir = ancestor / ".cargo"
        for name in CONFIG_NAMES:
            _reject_path(cargo_dir / name, "Cargo configuration")
    for name in CONFIG_NAMES:
        _reject_path(cargo_home / name, "Cargo-home configuration")


def reject_source_environment(environment: Mapping[str, str]) -> None:
    for raw_name in environment:
        name = raw_name.upper()
        if (
            name.startswith("CARGO_SOURCE_")
            or name == "CARGO_PATHS"
            or (name.startswith("CARGO_REGISTRIES_") and name.endswith("_INDEX"))
        ):
            raise ProvenanceError(f"forbidden Cargo source environment variable: {raw_name}")


def _read_manifest(path: Path) -> dict[str, object]:
    try:
        with path.open("rb") as manifest:
            parsed = tomllib.load(manifest)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ProvenanceError(f"cannot strictly parse {path}: {error}") from error
    if not isinstance(parsed, dict):
        raise ProvenanceError(f"manifest root is not a table: {path}")
    return parsed


def _reject_manifest_overrides(path: Path, manifest: Mapping[str, object]) -> None:
    for key in ("patch", "replace"):
        if key in manifest:
            raise ProvenanceError(f"forbidden [{key}] table in {path}")


def validate_workspace_manifests(root: Path) -> tuple[Path, ...]:
    root_manifest_path = root / "Cargo.toml"
    root_manifest = _read_manifest(root_manifest_path)
    _reject_manifest_overrides(root_manifest_path, root_manifest)

    workspace = root_manifest.get("workspace")
    if not isinstance(workspace, dict):
        raise ProvenanceError("root Cargo.toml has no [workspace] table")
    if workspace.get("resolver") != "3":
        raise ProvenanceError('root [workspace].resolver must be exactly "3"')
    members = workspace.get("members")
    if not isinstance(members, list) or not members:
        raise ProvenanceError("workspace members must be a non-empty explicit list")

    manifests: list[Path] = []
    seen: set[Path] = set()
    for member in members:
        if not isinstance(member, str) or not member:
            raise ProvenanceError("workspace member paths must be non-empty strings")
        member_path = Path(member)
        if member_path.is_absolute() or any(part in {".", ".."} for part in member_path.parts):
            raise ProvenanceError(f"workspace member path is not canonical: {member!r}")
        if any(character in member for character in "*?["):
            raise ProvenanceError(f"workspace member globs are forbidden: {member!r}")
        resolved_member = (root / member_path).resolve(strict=False)
        if not _is_within(resolved_member, root):
            raise ProvenanceError(f"workspace member escapes repository: {member!r}")
        manifest_path = resolved_member / "Cargo.toml"
        if manifest_path in seen:
            raise ProvenanceError(f"duplicate workspace member manifest: {manifest_path}")
        seen.add(manifest_path)
        manifest = _read_manifest(manifest_path)
        _reject_manifest_overrides(manifest_path, manifest)
        manifests.append(manifest_path)

    if not manifests:
        raise ProvenanceError("zero workspace member manifests were parsed")
    return tuple(manifests)


def validate_command(command: Sequence[str]) -> None:
    if not command or command[0] != "cargo":
        raise ProvenanceError("guard command must begin with exact executable name 'cargo'")
    for argument in command[1:]:
        if argument == "--config" or argument.startswith("--config="):
            raise ProvenanceError(f"Cargo global configuration argument is forbidden: {argument}")


def validate_invocation(
    root: Path,
    command: Sequence[str],
    environment: Mapping[str, str],
    cwd: Path,
    cargo_home: Path | None = None,
) -> None:
    canonical_root = root.resolve(strict=True)
    canonical_cwd = cwd.resolve(strict=True)
    if canonical_cwd != canonical_root:
        raise ProvenanceError(
            f"Cargo bootstrap must run from repository root {canonical_root}, got {canonical_cwd}"
        )
    validate_command(command)
    reject_source_environment(environment)
    effective_home = (cargo_home or active_cargo_home(environment, canonical_cwd)).resolve(
        strict=False
    )
    if _is_within(effective_home, canonical_root):
        raise ProvenanceError(
            f"active Cargo home must remain outside the repository: {effective_home}"
        )
    registry_source = effective_home / "registry" / "src"
    if registry_source.exists() or registry_source.is_symlink():
        physical_registry = registry_source.resolve(strict=False)
        if _is_within(physical_registry, canonical_root) or not _is_within(
            physical_registry, effective_home
        ):
            raise ProvenanceError(
                "Cargo registry source root escapes its trusted location: "
                f"{registry_source} -> {physical_registry}"
            )
    reject_cargo_configuration(canonical_root, effective_home)
    validate_workspace_manifests(canonical_root)


def _parse_command(arguments: Sequence[str]) -> tuple[str, ...]:
    if not arguments or arguments[0] != "--" or len(arguments) == 1:
        raise ProvenanceError("usage: source_provenance.py -- cargo <arguments>")
    return tuple(arguments[1:])


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        root = repository_root()
        ensure_runtime(root)
        command = _parse_command(tuple(sys.argv[1:] if arguments is None else arguments))
        validate_invocation(root, command, os.environ, Path.cwd())
        completed = subprocess.run(command, shell=False, check=False)
        return completed.returncode if completed.returncode >= 0 else 128 - completed.returncode
    except ProvenanceError as error:
        print(f"[source-provenance] {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
