"""Fail-closed Cargo source-provenance bootstrap for repository CI."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
from collections.abc import Mapping, Sequence


MINIMUM_PYTHON = (3, 11)
CONFIG_NAMES = ("config.toml", "config")
WORKFLOW_DIRECTORY = Path(".github/workflows")
WORKFLOW_NAME = "ci.yml"
WORKFLOW_SHA256 = "28da16df2f542db7ea435a1b4e4c9ab3eee41670408cbd313d94959cc0c708b3"
POLICY_PATH = Path("tools/xtask/dependency-surface-policy.v1.json")
DEPENDENCY_TABLES = (
    ("dependencies", "normal"),
    ("dev-dependencies", "dev"),
    ("build-dependencies", "build"),
)


class ProvenanceError(RuntimeError):
    """The Cargo invocation cannot produce an architecture verdict."""


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def _same_file(left: Path, right: Path) -> bool:
    try:
        return os.path.samefile(left, right)
    except OSError:
        return left == right


def _resolved(path: Path, *, base: Path | None = None) -> Path:
    candidate = path if path.is_absolute() else (base or Path.cwd()) / path
    return candidate.resolve(strict=False)


def _absolute(path: Path, *, base: Path | None = None) -> Path:
    candidate = path if path.is_absolute() else (base or Path.cwd()) / path
    return Path(os.path.abspath(candidate))


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
        return _absolute(Path(configured), base=cwd)
    return _absolute(Path.home() / ".cargo")


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


def validate_workflow_surface(root: Path) -> None:
    directory = root / WORKFLOW_DIRECTORY
    try:
        entries = sorted(directory.iterdir(), key=lambda entry: entry.name)
    except OSError as error:
        raise ProvenanceError(f"cannot enumerate workflow directory {directory}: {error}") from error
    names = [entry.name for entry in entries]
    if names != [WORKFLOW_NAME]:
        raise ProvenanceError(
            f"workflow directory must contain only {WORKFLOW_NAME}, found {names!r}"
        )
    workflow = entries[0]
    if workflow.is_symlink() or not workflow.is_file():
        raise ProvenanceError(f"workflow must be one unredirected regular file: {workflow}")
    try:
        digest = hashlib.sha256(workflow.read_bytes()).hexdigest()
    except OSError as error:
        raise ProvenanceError(f"cannot read workflow {workflow}: {error}") from error
    if digest != WORKFLOW_SHA256:
        raise ProvenanceError(
            f"workflow digest mismatch for {workflow}: expected {WORKFLOW_SHA256}, got {digest}"
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


def _require_unredirected_file(path: Path, root: Path, description: str) -> Path:
    lexical = _absolute(path)
    try:
        physical = path.resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"cannot resolve {description} {path}: {error}") from error
    if physical != lexical or not _is_within(physical, root) or not physical.is_file():
        raise ProvenanceError(
            f"redirected or external {description}: {lexical} -> {physical}"
        )
    return lexical


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


def _reject_workspace_build_script(
    package_root: Path,
    manifest: Mapping[str, object],
    description: str,
) -> None:
    package = manifest.get("package")
    if package is None:
        return
    package = _required_table(package, description)
    build_script = package_root / "build.rs"
    if (
        package.get("build", False) is not False
        or build_script.exists()
        or build_script.is_symlink()
    ):
        raise ProvenanceError(f"workspace build script is forbidden: {package_root}")


def _required_table(value: object, description: str) -> Mapping[str, object]:
    if not isinstance(value, dict):
        raise ProvenanceError(f"{description} is not a TOML table")
    return value


def _package_and_requirement(alias: str, value: object) -> tuple[str, str]:
    if isinstance(value, str):
        return alias, value
    table = _required_table(value, f"dependency {alias!r}")
    package = table.get("package", alias)
    requirement = table.get("version", "*")
    if not isinstance(package, str) or not package:
        raise ProvenanceError(f"dependency {alias!r} package must be a non-empty string")
    if not isinstance(requirement, str):
        raise ProvenanceError(f"dependency {alias!r} version must be a string")
    return package, requirement


def _authored_dependency(
    alias: str,
    value: object,
    workspace_dependencies: Mapping[str, object],
) -> tuple[str, str | None, str]:
    table = value if isinstance(value, dict) else None
    if table is not None and table.get("workspace") is True:
        if alias not in workspace_dependencies:
            raise ProvenanceError(
                f"workspace dependency {alias!r} has no [workspace.dependencies] owner"
            )
        if "package" in table or "version" in table:
            raise ProvenanceError(
                f"workspace dependency {alias!r} overrides package or version locally"
            )
        package, requirement = _package_and_requirement(
            alias, workspace_dependencies[alias]
        )
    else:
        package, requirement = _package_and_requirement(alias, value)
    rename = alias if alias != package else None
    return package, rename, requirement


def _dependency_table(
    owner: str,
    container: Mapping[str, object],
    table_name: str,
) -> Mapping[str, object]:
    alternate = table_name.replace("-", "_")
    spellings = (table_name, alternate) if alternate != table_name else (table_name,)
    values = [name for name in spellings if name in container]
    if len(values) > 1:
        raise ProvenanceError(
            f"workspace package {owner} declares both {table_name} spellings"
        )
    if not values:
        return {}
    return _required_table(container[values[0]], f"{owner} [{values[0]}]")


def _authored_requirements(
    manifest: Mapping[str, object],
    workspace_dependencies: Mapping[str, object],
) -> dict[tuple[str, str | None, str, str | None], str]:
    package = _required_table(manifest.get("package"), "workspace package")
    owner = package.get("name")
    if not isinstance(owner, str) or not owner:
        raise ProvenanceError("workspace package name must be a non-empty string")
    records: dict[tuple[str, str | None, str, str | None], str] = {}

    def collect(container: Mapping[str, object], kind: str, target: str | None) -> None:
        for table_name, table_kind in DEPENDENCY_TABLES:
            if table_kind != kind:
                continue
            for alias, value in _dependency_table(owner, container, table_name).items():
                package_name, rename, requirement = _authored_dependency(
                    alias, value, workspace_dependencies
                )
                key = (package_name, rename, kind, target)
                if key in records:
                    raise ProvenanceError(f"duplicate authored dependency identity: {owner} {key!r}")
                records[key] = requirement

    for table_name, kind in DEPENDENCY_TABLES:
        table = _dependency_table(owner, manifest, table_name)
        for alias, value in table.items():
            package_name, rename, requirement = _authored_dependency(
                alias, value, workspace_dependencies
            )
            key = (package_name, rename, kind, None)
            if key in records:
                raise ProvenanceError(f"duplicate authored dependency identity: {owner} {key!r}")
            records[key] = requirement

    targets = manifest.get("target", {})
    targets = _required_table(targets, f"workspace package {owner} target table")
    for target, target_value in targets.items():
        target_table = _required_table(target_value, f"workspace target {owner}/{target}")
        for _table_name, kind in DEPENDENCY_TABLES:
            collect(target_table, kind, target)
    return records


def validate_authored_requirements(
    root: Path,
    resolver: object,
    workspace_dependencies: Mapping[str, object],
    manifests: Sequence[Mapping[str, object]],
) -> None:
    policy_path = _require_unredirected_file(root / POLICY_PATH, root, "dependency policy")
    try:
        policy = json.loads(policy_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ProvenanceError(f"cannot parse dependency policy {policy_path}: {error}") from error
    if not isinstance(policy, dict) or policy.get("workspaceResolver") != resolver:
        raise ProvenanceError(
            f"workspace resolver does not match dependency policy: {resolver!r}"
        )
    packages = policy.get("packages")
    if not isinstance(packages, list):
        raise ProvenanceError("dependency policy packages must be an array")
    expected: dict[str, dict[tuple[str, str | None, str, str | None], str]] = {}
    for package in packages:
        package_table = _required_table(package, "dependency policy package")
        owner = package_table.get("name")
        dependencies = package_table.get("dependencies")
        if not isinstance(owner, str) or not isinstance(dependencies, list):
            raise ProvenanceError("dependency policy package has invalid name or dependencies")
        rows: dict[tuple[str, str | None, str, str | None], str] = {}
        for dependency in dependencies:
            row = _required_table(dependency, f"dependency policy row for {owner}")
            package_name = row.get("package")
            rename = row.get("rename")
            kind = row.get("kind")
            target = row.get("target")
            requirement = row.get("requirement")
            if (
                not isinstance(package_name, str)
                or (rename is not None and not isinstance(rename, str))
                or not isinstance(kind, str)
                or (target is not None and not isinstance(target, str))
                or not isinstance(requirement, str)
            ):
                raise ProvenanceError(f"dependency policy row has invalid identity: {owner}")
            key = (package_name, rename, kind, target)
            if key in rows:
                raise ProvenanceError(f"duplicate dependency policy identity: {owner} {key!r}")
            rows[key] = requirement
        if owner in expected:
            raise ProvenanceError(f"duplicate dependency policy package: {owner}")
        expected[owner] = rows

    observed: dict[str, dict[tuple[str, str | None, str, str | None], str]] = {}
    for manifest in manifests:
        package = _required_table(manifest.get("package"), "workspace package")
        owner = package.get("name")
        if not isinstance(owner, str) or owner in observed:
            raise ProvenanceError(f"invalid or duplicate workspace package name: {owner!r}")
        observed[owner] = _authored_requirements(manifest, workspace_dependencies)
    if observed != expected:
        raise ProvenanceError(
            f"authored dependency requirements drift: expected {expected!r}, observed {observed!r}"
        )


def validate_workspace_manifests(root: Path) -> tuple[Path, ...]:
    root_manifest_path = _require_unredirected_file(
        root / "Cargo.toml", root, "root workspace manifest"
    )
    root_manifest = _read_manifest(root_manifest_path)
    _reject_manifest_overrides(root_manifest_path, root_manifest)
    _reject_workspace_build_script(root, root_manifest, "root workspace package")

    workspace = root_manifest.get("workspace")
    if not isinstance(workspace, dict):
        raise ProvenanceError("root Cargo.toml has no [workspace] table")
    if workspace.get("resolver") != "3":
        raise ProvenanceError('root [workspace].resolver must be exactly "3"')
    workspace_dependencies = _required_table(
        workspace.get("dependencies", {}), "[workspace.dependencies]"
    )
    members = workspace.get("members")
    if not isinstance(members, list) or not members:
        raise ProvenanceError("workspace members must be a non-empty explicit list")

    manifests: list[Path] = []
    parsed_manifests: list[Mapping[str, object]] = []
    seen: set[Path] = set()
    for member in members:
        if not isinstance(member, str) or not member:
            raise ProvenanceError("workspace member paths must be non-empty strings")
        member_path = Path(member)
        if member_path.is_absolute() or any(part in {".", ".."} for part in member_path.parts):
            raise ProvenanceError(f"workspace member path is not canonical: {member!r}")
        if any(character in member for character in "*?["):
            raise ProvenanceError(f"workspace member globs are forbidden: {member!r}")
        lexical_member = root / member_path
        try:
            physical_member = lexical_member.resolve(strict=True)
        except OSError as error:
            raise ProvenanceError(f"cannot resolve workspace member {member!r}: {error}") from error
        if (
            physical_member != lexical_member
            or not _is_within(physical_member, root)
            or not physical_member.is_dir()
        ):
            raise ProvenanceError(
                f"redirected or external workspace member: {lexical_member} -> {physical_member}"
            )
        manifest_path = _require_unredirected_file(
            lexical_member / "Cargo.toml", root, "workspace member manifest"
        )
        if manifest_path in seen:
            raise ProvenanceError(f"duplicate workspace member manifest: {manifest_path}")
        seen.add(manifest_path)
        manifest = _read_manifest(manifest_path)
        _reject_manifest_overrides(manifest_path, manifest)
        _reject_workspace_build_script(
            lexical_member, manifest, f"workspace member {member!r}"
        )
        manifests.append(manifest_path)
        parsed_manifests.append(manifest)

    if not manifests:
        raise ProvenanceError("zero workspace member manifests were parsed")
    validate_authored_requirements(
        root,
        workspace.get("resolver"),
        workspace_dependencies,
        parsed_manifests,
    )
    return tuple(manifests)


def validate_command(command: Sequence[str]) -> None:
    if not command or command[0] != "cargo":
        raise ProvenanceError("guard command must begin with exact executable name 'cargo'")
    for argument in command[1:]:
        if argument == "--config" or argument.startswith("--config="):
            raise ProvenanceError(f"Cargo global configuration argument is forbidden: {argument}")


def validate_registry_root_identity(
    lexical_home: Path,
    physical_home: Path,
    lexical_registry: Path,
    physical_registry: Path,
) -> None:
    if physical_home != lexical_home:
        raise ProvenanceError(
            f"Cargo home lexical/physical disagreement: {lexical_home} -> {physical_home}"
        )
    if physical_registry != lexical_registry:
        raise ProvenanceError(
            "Cargo registry source root lexical/physical disagreement: "
            f"{lexical_registry} -> {physical_registry}"
        )


def validate_repository(
    root: Path,
    environment: Mapping[str, str],
    cwd: Path,
    cargo_home: Path | None = None,
) -> None:
    canonical_root = root.resolve(strict=True)
    canonical_cwd = cwd.resolve(strict=True)
    if not _same_file(canonical_cwd, canonical_root):
        raise ProvenanceError(
            f"Cargo bootstrap must run from repository root {canonical_root}, got {canonical_cwd}"
        )
    reject_source_environment(environment)
    lexical_home = _absolute(
        cargo_home or active_cargo_home(environment, canonical_cwd),
        base=canonical_cwd,
    )
    effective_home = lexical_home.resolve(strict=False)
    if _is_within(effective_home, canonical_root):
        raise ProvenanceError(
            f"active Cargo home must remain outside the repository: {effective_home}"
        )
    registry_source = lexical_home / "registry" / "src"
    if registry_source.exists() or registry_source.is_symlink():
        physical_registry = registry_source.resolve(strict=False)
        validate_registry_root_identity(
            lexical_home,
            effective_home,
            registry_source,
            physical_registry,
        )
        if _is_within(physical_registry, canonical_root) or not _is_within(
            physical_registry, effective_home
        ):
            raise ProvenanceError(
                "Cargo registry source root escapes its trusted location: "
                f"{registry_source} -> {physical_registry}"
            )
    else:
        validate_registry_root_identity(
            lexical_home,
            effective_home,
            registry_source,
            registry_source,
        )
    reject_cargo_configuration(canonical_root, lexical_home)
    validate_workflow_surface(canonical_root)
    validate_workspace_manifests(canonical_root)


def validate_invocation(
    root: Path,
    command: Sequence[str],
    environment: Mapping[str, str],
    cwd: Path,
    cargo_home: Path | None = None,
) -> None:
    validate_command(command)
    validate_repository(root, environment, cwd, cargo_home)


def _parse_command(arguments: Sequence[str]) -> tuple[str, ...]:
    if not arguments or arguments[0] != "--" or len(arguments) == 1:
        raise ProvenanceError("usage: source_provenance.py -- cargo <arguments>")
    return tuple(arguments[1:])


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        root = repository_root()
        ensure_runtime(root)
        raw_arguments = tuple(sys.argv[1:] if arguments is None else arguments)
        if raw_arguments == ("--check-only",):
            validate_repository(root, os.environ, Path.cwd())
            return 0
        command = _parse_command(raw_arguments)
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
