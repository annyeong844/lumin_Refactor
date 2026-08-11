#!/usr/bin/env python3
"""Fail-closed Cargo dependency admission for public CI.

The guard owns only the frozen REVIEW-003 surface: workspace membership and
features, authored direct dependency declarations, their Cargo-resolved direct
bindings, and loaded registry locations. Cargo.lock remains the sole transitive
graph pin. The script uses only the Python standard library and never writes the
checked-in policy.
"""

from __future__ import annotations

from collections import Counter
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
from typing import Any


MINIMUM_PYTHON = (3, 11)
EXPECTED_PYTHON = (3, 13, 14)
EXPECTED_CARGO_RELEASE = "1.96.0"
EXPECTED_CLIPPY_VERSION = "clippy 0.1.96 (ac68faa20c 2026-05-25)"
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
POLICY_PATH = Path("tools/xtask/dependency-surface-policy.v2.json")
CONFIG_NAMES = ("config.toml", "config")
ALLOWED_SUBCOMMANDS = frozenset(
    {"build", "check", "test", "clippy", "doc", "run", "bench", "metadata"}
)
FORBIDDEN_ENVIRONMENT = frozenset(
    {
        "CARGO",
        "CARGO_BUILD_TARGET",
        "CARGO_PATHS",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTUP_TOOLCHAIN",
    }
)
FORBIDDEN_LONG_OPTIONS = (
    "--config",
    "--manifest-path",
    "--lockfile-path",
    "--directory",
)
DEPENDENCY_TABLES = (
    ("dependencies", "normal"),
    ("build-dependencies", "build"),
    ("dev-dependencies", "development"),
)


class ProvenanceError(RuntimeError):
    """One owned dependency-admission failure."""


@dataclass(frozen=True)
class Member:
    name: str
    path: str
    directory: Path
    manifest_path: Path
    manifest: dict[str, Any]
    member_class: str


@dataclass(frozen=True)
class Repository:
    root: Path
    cargo_home: Path
    root_manifest: dict[str, Any]
    members: tuple[Member, ...]
    workspace_dependencies: tuple[dict[str, Any], ...]


@dataclass(frozen=True)
class CommandPlan:
    command: tuple[str, ...]
    subcommand: str | None
    explicit_target: str | None
    resolving: bool


@dataclass(frozen=True)
class MetadataView:
    raw: dict[str, Any]
    packages: dict[str, dict[str, Any]]
    member_ids: dict[str, str]
    nodes: dict[str, dict[str, Any]]


def repository_root() -> Path:
    return Path(__file__).absolute().parents[3]


def ensure_runtime() -> None:
    if sys.version_info < MINIMUM_PYTHON:
        raise ProvenanceError("Python 3.11 or newer is required")
    if sys.version_info[:3] != EXPECTED_PYTHON:
        raise ProvenanceError(
            f"expected pinned Python {'.'.join(map(str, EXPECTED_PYTHON))}, "
            f"got {sys.version.split()[0]}"
        )
    if not sys.flags.isolated or not sys.flags.no_site:
        raise ProvenanceError("invoke source_provenance.py with Python -I -S")


def _path_key(path: Path) -> str:
    return os.path.normcase(os.path.abspath(path))


def _same_path(left: Path, right: Path) -> bool:
    return _path_key(left) == _path_key(right)


def _inside(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def _absolute(path: Path, base: Path | None = None) -> Path:
    if not path.is_absolute():
        path = (base or Path.cwd()) / path
    return Path(os.path.abspath(path))


def _environment(environment: Mapping[str, str], case_insensitive: bool | None = None) -> dict[str, str]:
    case_insensitive = os.name == "nt" if case_insensitive is None else case_insensitive
    folded: dict[str, tuple[str, str]] = {}
    for name, value in environment.items():
        key = name.casefold() if case_insensitive else name
        previous = folded.get(key)
        if previous is not None and previous[0] != name:
            raise ProvenanceError(
                f"ambiguous environment names {previous[0]!r} and {name!r}"
            )
        folded[key] = (name, value)
    return {
        name.upper() if case_insensitive else name: value
        for name, value in environment.items()
    }


def reject_environment_overrides(
    environment: Mapping[str, str], case_insensitive: bool | None = None
) -> None:
    case_insensitive = os.name == "nt" if case_insensitive is None else case_insensitive
    _environment(environment, case_insensitive)
    for original in environment:
        name = original.upper() if case_insensitive else original
        if name in FORBIDDEN_ENVIRONMENT:
            raise ProvenanceError(f"forbidden Cargo/Rust environment override: {original}")
        if name.startswith("CARGO_SOURCE_") or name.startswith("CARGO_ALIAS_"):
            raise ProvenanceError(f"forbidden Cargo environment override: {original}")
        if name.startswith("CARGO_REGISTRIES_") and name.endswith("_INDEX"):
            raise ProvenanceError(f"forbidden registry index override: {original}")


def _env_get(environment: Mapping[str, str], name: str) -> str | None:
    if os.name != "nt":
        return environment.get(name)
    wanted = name.casefold()
    matches = [(key, value) for key, value in environment.items() if key.casefold() == wanted]
    if len(matches) > 1:
        raise ProvenanceError(f"ambiguous environment variable {name}")
    return matches[0][1] if matches else None


def validate_environment(environment: Mapping[str, str], root: Path, cwd: Path) -> Path:
    reject_environment_overrides(environment)

    raw_home = _env_get(environment, "CARGO_HOME")
    cargo_home = _absolute(
        Path(raw_home) if raw_home else Path.home() / ".cargo", base=cwd
    )
    physical_home = cargo_home.resolve(strict=False)
    if not _same_path(cargo_home, physical_home):
        raise ProvenanceError(f"active Cargo home is redirected: {cargo_home}")
    if _inside(physical_home, root):
        raise ProvenanceError(f"active Cargo home is repository-owned: {cargo_home}")

    target_value = _env_get(environment, "CARGO_TARGET_DIR")
    if target_value:
        target = _absolute(Path(target_value), base=cwd).resolve(strict=False)
        if _inside(target, root):
            raise ProvenanceError(f"Cargo target directory is repository-owned: {target}")

    if _env_get(environment, "GITHUB_ACTIONS") == "true":
        runner_value = _env_get(environment, "RUNNER_TEMP")
        if not runner_value:
            raise ProvenanceError("GitHub Actions requires RUNNER_TEMP")
        runner = _absolute(Path(runner_value), base=cwd)
        if not _same_path(runner, runner.resolve(strict=False)) or _inside(runner, root):
            raise ProvenanceError(f"GitHub runner temp is redirected or unsafe: {runner}")
        expected_home = runner / "lumin-cargo-home"
        expected_target = runner / "lumin-target"
        if not _same_path(cargo_home, expected_home):
            raise ProvenanceError(
                f"GitHub Cargo home must be job-private {expected_home}, got {cargo_home}"
            )
        if not target_value or not _same_path(
            _absolute(Path(target_value), base=cwd), expected_target
        ):
            raise ProvenanceError(
                f"GitHub Cargo target must be job-private {expected_target}"
            )
    return cargo_home


def _unredirected_file(path: Path, root: Path, label: str) -> Path:
    lexical = _absolute(path)
    try:
        physical = lexical.resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"cannot resolve {label} {lexical}: {error}") from error
    if not physical.is_file() or not _same_path(lexical, physical):
        raise ProvenanceError(f"{label} is missing or redirected: {lexical}")
    if not _inside(physical, root):
        raise ProvenanceError(f"{label} escapes repository: {lexical}")
    return physical


def _read_toml(path: Path, root: Path, label: str) -> dict[str, Any]:
    path = _unredirected_file(path, root, label)
    try:
        value = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise ProvenanceError(f"cannot parse {label} {path}: {error}") from error
    if not isinstance(value, dict):
        raise ProvenanceError(f"{label} must be a TOML table")
    return value


def reject_cargo_configuration(root: Path, cargo_home: Path) -> None:
    directories = (root, *root.parents)
    for directory in directories:
        for name in CONFIG_NAMES:
            candidate = directory / ".cargo" / name
            if candidate.exists() or candidate.is_symlink():
                raise ProvenanceError(f"Cargo configuration is forbidden: {candidate}")
    for name in CONFIG_NAMES:
        candidate = cargo_home / name
        if candidate.exists() or candidate.is_symlink():
            raise ProvenanceError(f"Cargo home configuration is forbidden: {candidate}")


def _table(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ProvenanceError(f"{label} must be a table")
    return value


def _string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ProvenanceError(f"{label} must be a nonempty string")
    return value


def _feature_set(value: Any, label: str) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ProvenanceError(f"{label} must be an array of strings")
    return sorted(set(value))


def _requested_features(table: Mapping[str, Any], label: str) -> list[str] | None:
    if "features" not in table:
        return None
    return _feature_set(table["features"], label)


def _feature_map(manifest: Mapping[str, Any], label: str) -> dict[str, list[str]]:
    raw = manifest.get("features", {})
    raw = _table(raw, f"{label} [features]")
    return {
        _string(name, f"{label} feature name"): _feature_set(
            value, f"{label} feature {name!r}"
        )
        for name, value in sorted(raw.items())
    }


def _default_features(table: Mapping[str, Any], label: str) -> bool:
    keys = [key for key in ("default-features", "default_features") if key in table]
    if len(keys) > 1:
        raise ProvenanceError(f"{label} declares both default-feature spellings")
    value = table.get(keys[0], True) if keys else True
    if not isinstance(value, bool):
        raise ProvenanceError(f"{label} default-features must be boolean")
    return value


def _dependency_value(value: Any, label: str) -> dict[str, Any]:
    if isinstance(value, str):
        return {"version": value}
    return dict(_table(value, label))


def _dependency_table(container: Mapping[str, Any], name: str, label: str) -> dict[str, Any]:
    alternate = name.replace("-", "_")
    candidates = (name,) if alternate == name else (name, alternate)
    present = [key for key in candidates if key in container]
    if len(present) > 1:
        raise ProvenanceError(f"{label} declares both {name} and {alternate}")
    if not present:
        return {}
    return _table(container[present[0]], f"{label} [{present[0]}]")


def _source_spec(
    alias: str,
    table: Mapping[str, Any],
    base: Path,
    members_by_directory: Mapping[str, Member],
    label: str,
) -> tuple[str, str | None, str]:
    if "git" in table or "registry" in table:
        raise ProvenanceError(f"{label} uses a forbidden Git or alternate-registry source")
    package = _string(table.get("package", alias), f"{label} package")
    path_value = table.get("path")
    requirement = table.get("version")
    if requirement is not None and not isinstance(requirement, str):
        raise ProvenanceError(f"{label} version requirement must be a string")
    if path_value is None:
        if requirement is None:
            raise ProvenanceError(f"{label} crates.io dependency requires an authored version")
        return package, requirement, "crates-io"
    path_text = _string(path_value, f"{label} path")
    lexical = _absolute(base / path_text)
    try:
        physical = lexical.resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"cannot resolve {label} path {lexical}: {error}") from error
    member = members_by_directory.get(_path_key(physical))
    if member is None or not _same_path(lexical, physical):
        raise ProvenanceError(f"{label} path does not resolve to an exact workspace member")
    if package != member.name:
        raise ProvenanceError(
            f"{label} package {package!r} disagrees with workspace member {member.name!r}"
        )
    return package, requirement, "workspace"


def _catalog_entry(
    alias: str,
    value: Any,
    root: Path,
    members_by_directory: Mapping[str, Member],
) -> dict[str, Any]:
    label = f"workspace dependency {alias!r}"
    table = _dependency_value(value, label)
    allowed = {
        "version", "package", "path", "git", "registry", "branch", "tag", "rev",
        "default-features", "default_features", "features",
    }
    unknown = sorted(set(table) - allowed)
    if unknown:
        raise ProvenanceError(f"{label} has unsupported keys: {', '.join(unknown)}")
    package, requirement, source_kind = _source_spec(
        alias, table, root, members_by_directory, label
    )
    entry: dict[str, Any] = {
        "alias": alias,
        "package": package,
        "requirement": requirement,
        "defaultFeatures": _default_features(table, label),
        "features": _requested_features(table, f"{label} features"),
        "sourceKind": source_kind,
    }
    if source_kind == "workspace":
        target = (_absolute(root / _string(table["path"], f"{label} path"))).resolve()
        entry["member"] = members_by_directory[_path_key(target)].name
    return entry


def _member_declaration(
    member: Member,
    alias: str,
    value: Any,
    kind: str,
    target: str | None,
    catalog: Mapping[str, dict[str, Any]],
    members_by_directory: Mapping[str, Member],
) -> dict[str, Any]:
    label = f"{member.name} {kind} dependency {alias!r}"
    table = _dependency_value(value, label)
    inherited = table.get("workspace") is True
    if "workspace" in table and table.get("workspace") is not True:
        raise ProvenanceError(f"{label} workspace flag must be true")
    if inherited:
        allowed = {"workspace", "optional", "default-features", "default_features", "features"}
        unknown = sorted(set(table) - allowed)
        if unknown:
            raise ProvenanceError(f"{label} has unsupported inherited keys: {', '.join(unknown)}")
        base = catalog.get(alias)
        if base is None:
            raise ProvenanceError(f"{label} has no matching workspace dependency")
        package = base["package"]
        requirement = base["requirement"]
        default_features = (
            _default_features(table, label)
            if "default-features" in table or "default_features" in table
            else base["defaultFeatures"]
        )
        features = _requested_features(table, f"{label} features")
        effective_features = sorted(set(base["features"] or []) | set(features or []))
        source_kind = base["sourceKind"]
        origin = "workspace-inherited"
    else:
        allowed = {
            "version", "package", "path", "git", "registry", "branch", "tag", "rev",
            "optional", "default-features", "default_features", "features",
        }
        unknown = sorted(set(table) - allowed)
        if unknown:
            raise ProvenanceError(f"{label} has unsupported keys: {', '.join(unknown)}")
        package, requirement, source_kind = _source_spec(
            alias, table, member.directory, members_by_directory, label
        )
        default_features = _default_features(table, label)
        features = _requested_features(table, f"{label} features")
        effective_features = features or []
        origin = "member-authored"
    optional = table.get("optional", False)
    if not isinstance(optional, bool):
        raise ProvenanceError(f"{label} optional must be boolean")
    return {
        "origin": origin,
        "kind": kind,
        "target": target,
        "alias": alias,
        "package": package,
        "requirement": requirement,
        "optional": optional,
        "defaultFeatures": default_features,
        "features": features,
        "sourceKind": source_kind,
        "_effectiveFeatures": effective_features,
    }


def _declarations(
    member: Member,
    catalog: Mapping[str, dict[str, Any]],
    members_by_directory: Mapping[str, Member],
) -> list[dict[str, Any]]:
    declarations: list[dict[str, Any]] = []

    def add_tables(container: Mapping[str, Any], target: str | None, label: str) -> None:
        for table_name, kind in DEPENDENCY_TABLES:
            table = _dependency_table(container, table_name, label)
            for alias, value in sorted(table.items()):
                declarations.append(
                    _member_declaration(
                        member,
                        _string(alias, f"{label} dependency alias"),
                        value,
                        kind,
                        target,
                        catalog,
                        members_by_directory,
                    )
                )

    add_tables(member.manifest, None, member.name)
    targets = _table(member.manifest.get("target", {}), f"{member.name} [target]")
    for target, target_value in sorted(targets.items()):
        target = _string(target, f"{member.name} target predicate")
        add_tables(_table(target_value, f"{member.name} target {target!r}"), target, member.name)
    declarations.sort(key=_json_key)
    keys = [_declaration_key(member.name, declaration) for declaration in declarations]
    duplicates = [key for key, count in Counter(keys).items() if count > 1]
    if duplicates:
        raise ProvenanceError(f"{member.name} has duplicate direct dependency declarations")
    return declarations


def inspect_repository(
    root: Path, environment: Mapping[str, str], cwd: Path
) -> Repository:
    root = _absolute(root)
    try:
        physical_root = root.resolve(strict=True)
        physical_cwd = _absolute(cwd).resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"cannot resolve repository root: {error}") from error
    if not _same_path(root, physical_root) or not _same_path(physical_root, physical_cwd):
        raise ProvenanceError(
            f"guard must run from the unredirected repository root {physical_root}"
        )
    cargo_home = validate_environment(environment, physical_root, physical_cwd)
    reject_cargo_configuration(physical_root, cargo_home)
    root_manifest = _read_toml(physical_root / "Cargo.toml", physical_root, "root manifest")
    _unredirected_file(physical_root / "Cargo.lock", physical_root, "root lockfile")
    if "package" in root_manifest:
        raise ProvenanceError("an implicit root workspace package is forbidden")
    if "patch" in root_manifest or "replace" in root_manifest:
        raise ProvenanceError("root [patch] and [replace] tables are forbidden")
    workspace = _table(root_manifest.get("workspace"), "root [workspace]")
    if workspace.get("resolver") != "3":
        raise ProvenanceError('root [workspace].resolver must be exactly "3"')
    raw_members = workspace.get("members")
    if not isinstance(raw_members, list) or not raw_members:
        raise ProvenanceError("root [workspace].members must be a nonempty array")

    members: list[Member] = []
    seen_paths: set[str] = set()
    seen_names: set[str] = set()
    for raw_path in raw_members:
        path = _string(raw_path, "workspace member path")
        if any(character in path for character in "*?["):
            raise ProvenanceError(f"workspace member globs are unsupported: {path}")
        lexical_directory = _absolute(physical_root / path)
        try:
            directory = lexical_directory.resolve(strict=True)
        except OSError as error:
            raise ProvenanceError(f"cannot resolve workspace member {path}: {error}") from error
        if not directory.is_dir() or not _inside(directory, physical_root) or not _same_path(
            lexical_directory, directory
        ):
            raise ProvenanceError(f"workspace member is external or redirected: {path}")
        manifest_path = _unredirected_file(
            directory / "Cargo.toml", physical_root, f"workspace member {path} manifest"
        )
        manifest = _read_toml(manifest_path, physical_root, f"workspace member {path}")
        if "patch" in manifest or "replace" in manifest:
            raise ProvenanceError(f"workspace member {path} has forbidden patch/replace")
        package = _table(manifest.get("package"), f"workspace member {path} [package]")
        name = _string(package.get("name"), f"workspace member {path} package name")
        path_key = _path_key(directory)
        if path_key in seen_paths or name in seen_names:
            raise ProvenanceError(f"duplicate workspace member path or name: {path} / {name}")
        seen_paths.add(path_key)
        seen_names.add(name)
        members.append(
            Member(
                name=name,
                path=Path(path).as_posix(),
                directory=directory,
                manifest_path=manifest_path,
                manifest=manifest,
                member_class="development-tool" if name == "lumin-xtask" else "production",
            )
        )
    if sum(member.member_class == "development-tool" for member in members) != 1:
        raise ProvenanceError("workspace must contain exactly one development tool lumin-xtask")

    members.sort(key=lambda member: member.name)
    members_by_directory = {_path_key(member.directory): member for member in members}
    raw_catalog = _table(workspace.get("dependencies", {}), "[workspace.dependencies]")
    catalog = tuple(
        _catalog_entry(alias, value, physical_root, members_by_directory)
        for alias, value in sorted(raw_catalog.items())
    )
    return Repository(
        root=physical_root,
        cargo_home=cargo_home,
        root_manifest=root_manifest,
        members=tuple(members),
        workspace_dependencies=catalog,
    )


def validate_command(command: Sequence[str]) -> CommandPlan:
    command = tuple(command)
    if not command or command[0] != "cargo":
        raise ProvenanceError("guarded commands must begin with the logical cargo token")
    if command == ("cargo", "--version"):
        return CommandPlan(command, None, None, False)
    if len(command) < 2:
        raise ProvenanceError("Cargo subcommand is missing")
    before = command[1 : command.index("--") + 1] if "--" in command[1:] else command[1:]
    before = before[:-1] if before and before[-1] == "--" else before
    subcommand = before[0] if before else ""
    if subcommand.startswith("+"):
        raise ProvenanceError("rustup +toolchain overrides are forbidden")
    if subcommand not in ALLOWED_SUBCOMMANDS:
        raise ProvenanceError(f"Cargo subcommand is not admitted: {subcommand or '<missing>'}")
    if sum(argument == "--locked" for argument in before) != 1:
        raise ProvenanceError("dependency-resolving Cargo commands require exactly one pre-delimiter --locked")
    for index, argument in enumerate(before):
        if argument.startswith("+"):
            raise ProvenanceError("rustup +toolchain overrides are forbidden")
        if argument in FORBIDDEN_LONG_OPTIONS or any(
            argument.startswith(option + "=") for option in FORBIDDEN_LONG_OPTIONS
        ):
            raise ProvenanceError(f"Cargo relocation option is forbidden: {argument}")
        if argument in {"-C", "-Z"} or (
            len(argument) > 2 and argument[:2] in {"-C", "-Z"}
        ):
            raise ProvenanceError(f"Cargo relocation/unstable option is forbidden: {argument}")
        if argument in FORBIDDEN_LONG_OPTIONS and index + 1 == len(before):
            raise ProvenanceError(f"Cargo option is missing its value: {argument}")
    equals_targets = [argument for argument in before if argument.startswith("--target=")]
    if equals_targets:
        raise ProvenanceError("Cargo --target=VALUE form is not admitted")
    targets: list[str] = []
    for index, argument in enumerate(before):
        if argument == "--target":
            if index + 1 >= len(before):
                raise ProvenanceError("Cargo --target is missing its value")
            targets.append(before[index + 1])
    if len(targets) > 1:
        raise ProvenanceError("duplicate Cargo --target options are forbidden")
    explicit_target = targets[0] if targets else None
    if explicit_target not in {None, "x86_64-unknown-linux-musl"}:
        raise ProvenanceError(f"unsupported Cargo target lane: {explicit_target}")
    return CommandPlan(command, subcommand, explicit_target, True)


def _pinned_path(
    environment: Mapping[str, str], name: str, root: Path, basename: str
) -> Path:
    raw = _env_get(environment, name)
    if not raw:
        raise ProvenanceError(f"missing {name}")
    path = Path(raw)
    if not path.is_absolute():
        raise ProvenanceError(f"{name} must be absolute: {raw}")
    try:
        physical = path.resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"cannot resolve {name} {path}: {error}") from error
    expected_names = {basename, basename + ".exe"}
    if not physical.is_file() or physical.name.casefold() not in {
        value.casefold() for value in expected_names
    }:
        raise ProvenanceError(f"{name} does not name {basename}: {physical}")
    if not _same_path(path, physical) or _inside(physical, root):
        raise ProvenanceError(f"{name} is redirected or repository-owned: {path}")
    return physical


def _run_text(command: Sequence[str], environment: Mapping[str, str], label: str) -> str:
    try:
        completed = subprocess.run(
            tuple(command),
            shell=False,
            check=False,
            capture_output=True,
            env=dict(environment),
        )
    except OSError as error:
        raise ProvenanceError(f"cannot run {label}: {error}") from error
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise ProvenanceError(f"{label} failed: {stderr or completed.returncode}")
    try:
        return completed.stdout.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ProvenanceError(f"{label} returned non-UTF-8 output") from error


def pinned_cargo(
    environment: Mapping[str, str], root: Path
) -> tuple[Path, str]:
    cargo = _pinned_path(environment, "PINNED_CARGO", root, "cargo")
    output = _run_text((str(cargo), "--version", "--verbose"), environment, "pinned Cargo probe")
    fields = {
        key.strip(): value.strip()
        for line in output.splitlines()
        if ":" in line
        for key, value in (line.split(":", 1),)
    }
    release = fields.get("release")
    host = fields.get("host")
    if release != EXPECTED_CARGO_RELEASE:
        raise ProvenanceError(f"pinned Cargo release mismatch: {release!r}")
    if host not in {"x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"}:
        raise ProvenanceError(f"unsupported pinned Cargo host: {host!r}")
    return cargo, host


def pinned_python(environment: Mapping[str, str], root: Path) -> None:
    raw = _env_get(environment, "PINNED_PYTHON")
    if not raw or not Path(raw).is_absolute():
        raise ProvenanceError("PINNED_PYTHON must name an absolute interpreter")
    # Microsoft Store app-execution aliases cannot be opened or resolved by the
    # calling process. They are acceptable for local diagnostics only; hosted CI
    # receives a normal setup-python path and takes the strict branch.
    if _env_get(environment, "GITHUB_ACTIONS") == "true":
        python = _pinned_path(environment, "PINNED_PYTHON", root, "python")
        running = Path(sys.executable).resolve(strict=True)
    else:
        python = _absolute(Path(raw))
        running = _absolute(Path(sys.executable))
        if python.name.casefold() not in {"python", "python.exe"}:
            raise ProvenanceError(f"PINNED_PYTHON does not name Python: {python}")
    if not _same_path(python, running):
        raise ProvenanceError(
            f"running interpreter {sys.executable} differs from PINNED_PYTHON {python}"
        )


def pinned_clippy(environment: Mapping[str, str], root: Path) -> Path:
    clippy = _pinned_path(environment, "PINNED_CARGO_CLIPPY", root, "cargo-clippy")
    version = _run_text(
        (str(clippy), "clippy", "--version"), environment, "pinned cargo-clippy probe"
    ).strip()
    if version != EXPECTED_CLIPPY_VERSION:
        raise ProvenanceError(f"pinned cargo-clippy version mismatch: {version!r}")
    return clippy


def _metadata_command(cargo: Path, target: str | None) -> tuple[str, ...]:
    command = [
        str(cargo), "metadata", "--format-version", "1", "--all-features", "--locked"
    ]
    if target is not None:
        command.extend(("--filter-platform", target))
    return tuple(command)


def run_metadata(
    cargo: Path,
    target: str | None,
    repository: Repository,
    environment: Mapping[str, str],
) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            _metadata_command(cargo, target),
            cwd=repository.root,
            env=dict(environment),
            shell=False,
            check=False,
            capture_output=True,
        )
    except OSError as error:
        raise ProvenanceError(f"cannot run pinned Cargo metadata: {error}") from error
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise ProvenanceError(f"pinned Cargo metadata failed: {stderr or completed.returncode}")
    try:
        value = json.loads(completed.stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ProvenanceError(f"pinned Cargo metadata returned invalid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ProvenanceError("pinned Cargo metadata root must be an object")
    return value


def _metadata_view(metadata: dict[str, Any], repository: Repository) -> MetadataView:
    root_value = metadata.get("workspace_root")
    if not isinstance(root_value, str) or not _same_path(
        Path(root_value).resolve(strict=True), repository.root
    ):
        raise ProvenanceError("Cargo metadata workspace root mismatch")
    raw_packages = metadata.get("packages")
    raw_members = metadata.get("workspace_members")
    resolve = metadata.get("resolve")
    if not isinstance(raw_packages, list) or not isinstance(raw_members, list) or not isinstance(resolve, dict):
        raise ProvenanceError("Cargo metadata package/member/resolve surface is incomplete")
    packages: dict[str, dict[str, Any]] = {}
    for package in raw_packages:
        if not isinstance(package, dict) or not isinstance(package.get("id"), str):
            raise ProvenanceError("Cargo metadata contains a malformed package")
        if package["id"] in packages:
            raise ProvenanceError(f"duplicate Cargo metadata package id: {package['id']}")
        packages[package["id"]] = package
    member_ids: dict[str, str] = {}
    for package_id in raw_members:
        if not isinstance(package_id, str) or package_id not in packages:
            raise ProvenanceError("Cargo metadata contains an unknown workspace member id")
        package = packages[package_id]
        name = package.get("name")
        if not isinstance(name, str) or name in member_ids:
            raise ProvenanceError("Cargo metadata contains duplicate/malformed member names")
        member_ids[name] = package_id
    expected = {member.name for member in repository.members}
    if set(member_ids) != expected:
        raise ProvenanceError(
            f"Cargo metadata workspace members differ: expected {sorted(expected)}, got {sorted(member_ids)}"
        )
    raw_nodes = resolve.get("nodes")
    if not isinstance(raw_nodes, list):
        raise ProvenanceError("Cargo metadata resolve.nodes is incomplete")
    nodes: dict[str, dict[str, Any]] = {}
    for node in raw_nodes:
        if not isinstance(node, dict) or not isinstance(node.get("id"), str):
            raise ProvenanceError("Cargo metadata contains a malformed resolve node")
        if node["id"] in nodes:
            raise ProvenanceError(f"duplicate Cargo metadata resolve node: {node['id']}")
        nodes[node["id"]] = node
    for member in repository.members:
        package = packages[member_ids[member.name]]
        manifest = package.get("manifest_path")
        if not isinstance(manifest, str) or not _same_path(
            Path(manifest).resolve(strict=True), member.manifest_path
        ):
            raise ProvenanceError(f"Cargo metadata manifest mismatch for {member.name}")
        if package.get("source") is not None:
            raise ProvenanceError(f"workspace package {member.name} has a non-workspace source")
        if member_ids[member.name] not in nodes:
            raise ProvenanceError(f"workspace member {member.name} has no resolve node")
    return MetadataView(metadata, packages, member_ids, nodes)


def _loaded_sources(view: MetadataView, repository: Repository) -> None:
    registry_root = repository.cargo_home / "registry" / "src"
    try:
        physical_registry = registry_root.resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"Cargo registry source root is unavailable: {error}") from error
    if not physical_registry.is_dir() or not _same_path(registry_root, physical_registry):
        raise ProvenanceError(f"Cargo registry source root is redirected: {registry_root}")
    if _inside(physical_registry, repository.root):
        raise ProvenanceError("Cargo registry source root is inside the repository")
    member_ids = set(view.member_ids.values())
    for package_id, package in view.packages.items():
        if package_id in member_ids:
            continue
        if package.get("source") != CRATES_IO_SOURCE:
            raise ProvenanceError(
                f"non-workspace package {package.get('name')!r} is not exact crates.io"
            )
        manifest_value = package.get("manifest_path")
        if not isinstance(manifest_value, str):
            raise ProvenanceError("registry package is missing manifest_path")
        manifest = Path(manifest_value)
        if not manifest.is_absolute():
            raise ProvenanceError(f"registry manifest is not absolute: {manifest}")
        try:
            physical_manifest = manifest.resolve(strict=True)
            physical_package = manifest.parent.resolve(strict=True)
        except OSError as error:
            raise ProvenanceError(f"cannot resolve registry manifest {manifest}: {error}") from error
        if not physical_manifest.is_file() or not _same_path(manifest, physical_manifest):
            raise ProvenanceError(f"registry manifest is redirected: {manifest}")
        if not _same_path(manifest.parent, physical_package):
            raise ProvenanceError(f"registry package directory is redirected: {manifest.parent}")
        if not _inside(physical_manifest, physical_registry) or _inside(
            physical_manifest, repository.root
        ):
            raise ProvenanceError(f"registry manifest escapes active Cargo home: {manifest}")


def _kind(value: Any) -> str:
    mapping = {None: "normal", "build": "build", "dev": "development"}
    if value not in mapping:
        raise ProvenanceError(f"unknown Cargo dependency kind: {value!r}")
    return mapping[value]


def _normalized_alias(alias: str) -> str:
    return alias.replace("-", "_")


def _resolution(package: Mapping[str, Any], view: MetadataView) -> dict[str, Any]:
    package_id = package.get("id")
    for member_name, member_id in view.member_ids.items():
        if package_id == member_id:
            return {"kind": "workspace", "member": member_name}
    name = package.get("name")
    version = package.get("version")
    source = package.get("source")
    if not isinstance(name, str) or not isinstance(version, str) or source != CRATES_IO_SOURCE:
        raise ProvenanceError("resolved third-party package identity is incomplete")
    return {"kind": "crates-io", "name": name, "version": version, "source": source}


def _declaration_key(owner: str, declaration: Mapping[str, Any]) -> tuple[Any, ...]:
    return (
        owner,
        declaration["origin"],
        declaration["kind"],
        declaration["target"],
        declaration["alias"],
        declaration["package"],
        declaration["requirement"],
        declaration["optional"],
        declaration["defaultFeatures"],
        None if declaration["features"] is None else tuple(declaration["features"]),
        declaration["sourceKind"],
    )


def _binding_key(owner: str, declaration: Mapping[str, Any]) -> tuple[Any, ...]:
    return (
        owner,
        _normalized_alias(declaration["alias"]),
        declaration["kind"],
        declaration["target"],
    )


def _resolved_bindings(view: MetadataView) -> dict[tuple[Any, ...], dict[str, Any]]:
    bindings: dict[tuple[Any, ...], dict[str, Any]] = {}
    for owner, package_id in view.member_ids.items():
        deps = view.nodes[package_id].get("deps")
        if not isinstance(deps, list):
            raise ProvenanceError(f"resolve deps are missing for {owner}")
        for dependency in deps:
            if not isinstance(dependency, dict):
                raise ProvenanceError(f"malformed resolve dependency for {owner}")
            alias = dependency.get("name")
            destination = dependency.get("pkg")
            dep_kinds = dependency.get("dep_kinds")
            if not isinstance(alias, str) or destination not in view.packages or not isinstance(dep_kinds, list):
                raise ProvenanceError(f"incomplete resolve dependency for {owner}")
            for dep_kind in dep_kinds:
                if not isinstance(dep_kind, dict):
                    raise ProvenanceError(f"malformed dependency kind for {owner}")
                target = dep_kind.get("target")
                if target is not None and not isinstance(target, str):
                    raise ProvenanceError(f"malformed dependency target for {owner}")
                key = (owner, alias, _kind(dep_kind.get("kind")), target)
                if key in bindings:
                    raise ProvenanceError(f"duplicate resolved direct binding: {key}")
                bindings[key] = _resolution(view.packages[destination], view)
    return bindings


def _verify_metadata_declarations(
    repository: Repository,
    authored: Mapping[str, list[dict[str, Any]]],
    view: MetadataView,
) -> None:
    for member in repository.members:
        package = view.packages[view.member_ids[member.name]]
        raw_dependencies = package.get("dependencies")
        if not isinstance(raw_dependencies, list):
            raise ProvenanceError(f"metadata dependencies are missing for {member.name}")
        actual: Counter[tuple[Any, ...]] = Counter()
        for dependency in raw_dependencies:
            if not isinstance(dependency, dict):
                raise ProvenanceError(f"malformed metadata dependency for {member.name}")
            package_name = dependency.get("name")
            rename = dependency.get("rename")
            alias = rename if isinstance(rename, str) else package_name
            source = dependency.get("source")
            source_kind = "crates-io" if source == CRATES_IO_SOURCE else "workspace"
            features = dependency.get("features")
            target = dependency.get("target")
            if not isinstance(package_name, str) or not isinstance(alias, str):
                raise ProvenanceError(f"metadata dependency name is missing for {member.name}")
            if not isinstance(features, list) or any(not isinstance(value, str) for value in features):
                raise ProvenanceError(f"metadata dependency features are malformed for {member.name}")
            actual[
                (
                    _normalized_alias(alias), package_name, _kind(dependency.get("kind")), target,
                    dependency.get("optional"), dependency.get("uses_default_features"),
                    tuple(sorted(set(features))), source_kind,
                )
            ] += 1
        expected: Counter[tuple[Any, ...]] = Counter(
            (
                _normalized_alias(declaration["alias"]), declaration["package"], declaration["kind"],
                declaration["target"], declaration["optional"], declaration["defaultFeatures"],
                tuple(declaration["_effectiveFeatures"]), declaration["sourceKind"],
            )
            for declaration in authored[member.name]
        )
        if actual != expected:
            raise ProvenanceError(f"Cargo metadata declaration surface differs for {member.name}")


def build_policy(repository: Repository, metadata: dict[str, Any]) -> dict[str, Any]:
    view = _metadata_view(metadata, repository)
    _loaded_sources(view, repository)
    catalog = {entry["alias"]: entry for entry in repository.workspace_dependencies}
    members_by_directory = {_path_key(member.directory): member for member in repository.members}
    authored = {
        member.name: _declarations(member, catalog, members_by_directory)
        for member in repository.members
    }
    _verify_metadata_declarations(repository, authored, view)
    bindings = _resolved_bindings(view)
    policy_members: list[dict[str, Any]] = []
    for member in repository.members:
        dependencies: list[dict[str, Any]] = []
        for declaration in authored[member.name]:
            key = _binding_key(member.name, declaration)
            resolution = bindings.pop(key, None)
            if resolution is None:
                raise ProvenanceError(f"unresolved direct dependency binding: {key}")
            dependency = {
                key: value
                for key, value in declaration.items()
                if not key.startswith("_")
            }
            dependency["resolution"] = resolution
            dependencies.append(dependency)
        dependencies.sort(key=_json_key)
        policy_members.append(
            {
                "name": member.name,
                "class": member.member_class,
                "path": member.path,
                "features": _feature_map(member.manifest, member.name),
                "dependencies": dependencies,
            }
        )
    if bindings:
        raise ProvenanceError(f"Cargo metadata has unowned direct bindings: {sorted(bindings)}")
    return {
        "schemaVersion": 2,
        "resolver": "3",
        "members": policy_members,
        "workspaceDependencies": list(repository.workspace_dependencies),
    }


def _target_applies(target: str | None, lane: str) -> bool:
    if target is None:
        return True
    if target == "cfg(windows)":
        return lane == "x86_64-pc-windows-msvc"
    raise ProvenanceError(f"unsupported target predicate in frozen policy: {target}")


def validate_filtered_lane(
    policy: Mapping[str, Any], metadata: dict[str, Any], repository: Repository, lane: str
) -> None:
    view = _metadata_view(metadata, repository)
    _loaded_sources(view, repository)
    actual = _resolved_bindings(view)
    expected: dict[tuple[Any, ...], dict[str, Any]] = {}
    members = policy.get("members")
    if not isinstance(members, list):
        raise ProvenanceError("policy members are malformed")
    for member in members:
        if not isinstance(member, dict) or not isinstance(member.get("dependencies"), list):
            raise ProvenanceError("policy member dependency surface is malformed")
        owner = member.get("name")
        if not isinstance(owner, str):
            raise ProvenanceError("policy member name is malformed")
        for declaration in member["dependencies"]:
            if not isinstance(declaration, dict):
                raise ProvenanceError("policy dependency declaration is malformed")
            if _target_applies(declaration.get("target"), lane):
                expected[_binding_key(owner, declaration)] = declaration.get("resolution")
    if actual != expected:
        raise ProvenanceError(f"filtered Cargo metadata direct bindings differ for lane {lane}")


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ProvenanceError(f"dependency policy repeats JSON key {key!r}")
        result[key] = value
    return result


def load_policy(root: Path) -> dict[str, Any]:
    path = _unredirected_file(root / POLICY_PATH, root, "dependency policy")
    try:
        policy = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_unique_object)
    except ProvenanceError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ProvenanceError(f"cannot parse dependency policy {path}: {error}") from error
    if not isinstance(policy, dict):
        raise ProvenanceError("dependency policy root must be an object")
    _validate_policy_shape(policy)
    return policy


def _exact_keys(value: Mapping[str, Any], expected: set[str], label: str) -> None:
    if set(value) != expected:
        raise ProvenanceError(
            f"{label} keys differ: expected {sorted(expected)}, got {sorted(value)}"
        )


def _validate_policy_shape(policy: Mapping[str, Any]) -> None:
    _exact_keys(policy, {"schemaVersion", "resolver", "members", "workspaceDependencies"}, "policy")
    if policy.get("schemaVersion") != 2 or policy.get("resolver") != "3":
        raise ProvenanceError("dependency policy schema/resolver mismatch")
    members = policy.get("members")
    catalog = policy.get("workspaceDependencies")
    if not isinstance(members, list) or not members or not isinstance(catalog, list):
        raise ProvenanceError("dependency policy member/catalog arrays are malformed")
    for entry in catalog:
        if not isinstance(entry, dict):
            raise ProvenanceError("workspace dependency policy entry must be an object")
        keys = {"alias", "package", "requirement", "defaultFeatures", "features", "sourceKind"}
        if entry.get("sourceKind") == "workspace":
            keys.add("member")
        _exact_keys(entry, keys, "workspace dependency policy entry")
    for member in members:
        if not isinstance(member, dict):
            raise ProvenanceError("member policy entry must be an object")
        _exact_keys(member, {"name", "class", "path", "features", "dependencies"}, "member policy entry")
        if member.get("class") not in {"production", "development-tool"}:
            raise ProvenanceError("member policy class is invalid")
        if not isinstance(member.get("features"), dict) or not isinstance(member.get("dependencies"), list):
            raise ProvenanceError("member feature/dependency policy is malformed")
        for dependency in member["dependencies"]:
            if not isinstance(dependency, dict):
                raise ProvenanceError("dependency policy entry must be an object")
            _exact_keys(
                dependency,
                {
                    "origin", "kind", "target", "alias", "package", "requirement", "optional",
                    "defaultFeatures", "features", "sourceKind", "resolution",
                },
                "dependency policy entry",
            )
            resolution = dependency.get("resolution")
            if not isinstance(resolution, dict):
                raise ProvenanceError("dependency resolution policy must be an object")
            if resolution.get("kind") == "workspace":
                _exact_keys(resolution, {"kind", "member"}, "workspace resolution")
            elif resolution.get("kind") == "crates-io":
                _exact_keys(resolution, {"kind", "name", "version", "source"}, "registry resolution")
            else:
                raise ProvenanceError("dependency resolution kind is invalid")


def _first_difference(expected: Any, actual: Any, path: str = "$") -> str | None:
    if type(expected) is not type(actual):
        return f"{path}: expected {type(expected).__name__}, got {type(actual).__name__}"
    if isinstance(expected, dict):
        if set(expected) != set(actual):
            return f"{path}: keys differ"
        for key in expected:
            difference = _first_difference(expected[key], actual[key], f"{path}.{key}")
            if difference:
                return difference
        return None
    if isinstance(expected, list):
        if len(expected) != len(actual):
            return f"{path}: expected {len(expected)} entries, got {len(actual)}"
        for index, (left, right) in enumerate(zip(expected, actual, strict=True)):
            difference = _first_difference(left, right, f"{path}[{index}]")
            if difference:
                return difference
        return None
    return None if expected == actual else f"{path}: expected {expected!r}, got {actual!r}"


def _json_key(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def _lane(host: str, explicit_target: str | None) -> str:
    if explicit_target is not None:
        if host != "x86_64-unknown-linux-gnu":
            raise ProvenanceError("Linux musl target is admitted only from the Linux GNU host")
        return explicit_target
    return host


def dependency_preflight(
    repository: Repository,
    cargo: Path,
    host: str,
    explicit_target: str | None,
    environment: Mapping[str, str],
    compare_policy: bool = True,
) -> dict[str, Any]:
    lane = _lane(host, explicit_target)
    unfiltered = run_metadata(cargo, None, repository, environment)
    actual_policy = build_policy(repository, unfiltered)
    policy = load_policy(repository.root) if compare_policy else actual_policy
    if compare_policy:
        difference = _first_difference(policy, actual_policy)
        if difference:
            raise ProvenanceError(f"dependency surface policy drift at {difference}")
    filtered = run_metadata(cargo, lane, repository, environment)
    validate_filtered_lane(policy, filtered, repository, lane)
    return actual_policy


def resolved_command(
    plan: CommandPlan,
    cargo: Path,
    environment: Mapping[str, str],
    root: Path,
) -> tuple[str, ...]:
    if plan.subcommand == "clippy":
        clippy = pinned_clippy(environment, root)
        return (str(clippy), "clippy", *plan.command[2:])
    return (str(cargo), *plan.command[1:])


def _parse_command(arguments: Sequence[str]) -> tuple[str, ...]:
    if not arguments or arguments[0] != "--" or len(arguments) == 1:
        raise ProvenanceError("usage: source_provenance.py -- cargo <arguments>")
    return tuple(arguments[1:])


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        ensure_runtime()
        root = repository_root()
        raw = tuple(sys.argv[1:] if arguments is None else arguments)
        mode = "command"
        if raw == ("--check-only",):
            mode = "check"
            plan = CommandPlan(("cargo", "metadata", "--locked"), "metadata", None, True)
        elif raw == ("--print-policy",):
            mode = "print"
            plan = CommandPlan(("cargo", "metadata", "--locked"), "metadata", None, True)
        else:
            plan = validate_command(_parse_command(raw))
        pinned_python(os.environ, root)
        repository = inspect_repository(root, os.environ, Path.cwd())
        cargo, host = pinned_cargo(os.environ, root)
        if plan.resolving:
            policy = dependency_preflight(
                repository,
                cargo,
                host,
                plan.explicit_target,
                os.environ,
                compare_policy=mode != "print",
            )
            if mode == "print":
                print(json.dumps(policy, indent=2, ensure_ascii=False))
                return 0
        if mode == "check":
            print("[source-provenance] dependency admission PASS")
            return 0
        command = resolved_command(plan, cargo, os.environ, root)
        completed = subprocess.run(command, shell=False, check=False, env=os.environ.copy())
        return completed.returncode if completed.returncode >= 0 else 128 - completed.returncode
    except ProvenanceError as error:
        print(f"[source-provenance] {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
