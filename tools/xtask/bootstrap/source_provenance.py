"""Fail-closed Cargo source-provenance bootstrap for repository CI."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib
from collections.abc import Callable, Mapping, Sequence


MINIMUM_PYTHON = (3, 11)
CONFIG_NAMES = ("config.toml", "config")
WORKFLOW_DIRECTORY = Path(".github/workflows")
WORKFLOW_NAME = "ci.yml"
WORKFLOW_SHA256 = "4ca2610501059a8ae6eacffd0b93547cb69115c221460eb012b2aa5465dbfc4c"
POLICY_PATH = Path("tools/xtask/dependency-surface-policy.v1.json")
METADATA_HELPER_PATH = Path("tools/xtask/bootstrap/metadata_snapshot.py")
METADATA_HELPER_SHA256 = "dc23605129c4fe78dd197804fa80466a602de921558880fcb848f1870963fdae"
REGISTRY_HELPER_PATH = Path("tools/xtask/bootstrap/registry_snapshot.py")
REGISTRY_HELPER_SHA256 = "4a02777fd52f116007ca53d0aa2d4989c447fe27261d64531ee449013dda8857"
DEPENDENCY_TABLES = (
    ("dependencies", "normal"),
    ("dev-dependencies", "dev"),
    ("build-dependencies", "build"),
)
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
FORBIDDEN_DEPENDENCY_SOURCE_KEYS = frozenset(
    {"git", "registry", "branch", "tag", "rev"}
)
FORBIDDEN_ENVIRONMENT_EXACT = frozenset(
    {
        "CARGO",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTDOC",
        "RUSTFMT",
        "CLIPPY_DRIVER",
        "RUSTFLAGS",
        "RUSTDOCFLAGS",
        "RUSTC_BOOTSTRAP",
        "RUSTUP_TOOLCHAIN",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
    }
)
FORBIDDEN_ENVIRONMENT_PREFIXES = (
    "CARGO_UNSTABLE_",
    "CARGO_PROFILE_",
    "CARGO_ALIAS_",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTDOC",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_TARGET_",
)
GITHUB_COMMAND_FILE_ENVIRONMENT = frozenset(
    {
        "GITHUB_ENV",
        "GITHUB_PATH",
        "GITHUB_OUTPUT",
        "GITHUB_STATE",
        "GITHUB_STEP_SUMMARY",
    }
)
TERMINAL_CARGO_SUBCOMMANDS = frozenset({"bench", "run", "test"})


class ProvenanceError(RuntimeError):
    """The Cargo invocation cannot produce an architecture verdict."""


@dataclass(frozen=True)
class DependencyContract:
    requirement: str
    resolution_kind: str
    optional: bool
    uses_default_features: bool
    features: tuple[str, ...]


@dataclass(frozen=True)
class RepositoryValidation:
    root: Path
    cargo_home: Path
    manifests: tuple[Path, ...]


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
    return _portable_windows_path(candidate.resolve(strict=False))


def _portable_windows_path(path: Path) -> Path:
    if os.name != "nt":
        return path
    value = str(path)
    if value.upper().startswith("\\\\?\\UNC\\"):
        return Path("\\\\" + value[8:])
    if (
        value.startswith("\\\\?\\")
        and len(value) >= 7
        and value[4].isalpha()
        and value[5:7] == ":\\"
    ):
        return Path(value[4:])
    return path


def _absolute(path: Path, *, base: Path | None = None) -> Path:
    candidate = path if path.is_absolute() else (base or Path.cwd()) / path
    return _portable_windows_path(Path(os.path.abspath(candidate)))


def _environment_value(
    environment: Mapping[str, str], name: str
) -> str | None:
    matches = [
        value for raw_name, value in environment.items() if raw_name.upper() == name
    ]
    if len(matches) > 1:
        raise ProvenanceError(f"duplicate case-insensitive environment key: {name}")
    return matches[0] if matches else None


def repository_root() -> Path:
    root = _absolute(Path(__file__).resolve()).parents[3]
    if not (root / "Cargo.toml").is_file():
        raise ProvenanceError(f"repository Cargo.toml is missing under {root}")
    return root


def active_cargo_home(environment: Mapping[str, str], cwd: Path) -> Path:
    configured = _environment_value(environment, "CARGO_HOME")
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
    for raw_name, value in environment.items():
        name = raw_name.upper()
        if (
            name.startswith("CARGO_SOURCE_")
            or name == "CARGO_PATHS"
            or (name.startswith("CARGO_REGISTRIES_") and name.endswith("_INDEX"))
        ):
            raise ProvenanceError(f"forbidden Cargo source environment variable: {raw_name}")
        if name == "CARGO_TARGET_DIR":
            if not value:
                raise ProvenanceError("CARGO_TARGET_DIR must not be empty")
            continue
        if name == "CARGO_INCREMENTAL":
            if value != "0":
                raise ProvenanceError("CARGO_INCREMENTAL must be exactly 0 when provided")
            continue
        if name in FORBIDDEN_ENVIRONMENT_EXACT or name.startswith(
            FORBIDDEN_ENVIRONMENT_PREFIXES
        ):
            raise ProvenanceError(
                f"forbidden Cargo or compiler environment variable: {raw_name}"
            )
        if os.name == "nt" and name in {"LINK", "_LINK_", "LIB"}:
            raise ProvenanceError(
                f"inherited MSVC linker environment variable is forbidden: {raw_name}"
            )


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


def _unique_json_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            raise ProvenanceError(f"duplicate dependency policy JSON key: {key!r}")
        value[key] = item
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


def _dependency_features(alias: str, table: Mapping[str, object] | None) -> tuple[str, ...]:
    if table is None:
        return ()
    raw_features = table.get("features", [])
    if not isinstance(raw_features, list) or any(
        not isinstance(feature, str) or not feature for feature in raw_features
    ):
        raise ProvenanceError(f"dependency {alias!r} features must be non-empty strings")
    return tuple(sorted(set(raw_features)))


def canonical_workspace_dependency_catalog(
    dependencies: Mapping[str, object],
) -> dict[str, object]:
    catalog: dict[str, object] = {}
    for alias, value in dependencies.items():
        if not isinstance(value, dict) or "features" not in value:
            catalog[alias] = value
            continue
        normalized = dict(value)
        normalized["features"] = list(_dependency_features(alias, value))
        catalog[alias] = normalized
    return catalog


def _dependency_bool(
    alias: str,
    table: Mapping[str, object] | None,
    key: str,
    default: bool,
) -> bool:
    if table is None or key not in table:
        return default
    value = table[key]
    if not isinstance(value, bool):
        raise ProvenanceError(f"dependency {alias!r} {key} must be a boolean")
    return value


def _dependency_resolution_kind(
    alias: str,
    package: str,
    table: Mapping[str, object] | None,
    base: Path,
    workspace_members: Mapping[Path, str],
) -> str:
    if table is None:
        return "third-party"
    forbidden = sorted(FORBIDDEN_DEPENDENCY_SOURCE_KEYS.intersection(table))
    if forbidden:
        raise ProvenanceError(
            f"dependency {alias!r} uses forbidden source selectors: {forbidden!r}"
        )
    if "path" not in table:
        return "third-party"
    raw_path = table["path"]
    if not isinstance(raw_path, str) or not raw_path:
        raise ProvenanceError(f"dependency {alias!r} path must be a non-empty string")
    candidate = Path(raw_path)
    if candidate.is_absolute():
        raise ProvenanceError(f"dependency {alias!r} path must be repository-relative")
    lexical = _absolute(base / candidate)
    try:
        physical = (base / candidate).resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(
            f"cannot resolve dependency {alias!r} path {raw_path!r}: {error}"
        ) from error
    target_package = workspace_members.get(physical)
    if physical != lexical or target_package is None or target_package != package:
        raise ProvenanceError(
            "dependency path must resolve directly to its exact workspace member: "
            f"{alias!r} ({package}) {lexical} -> {physical}"
        )
    return "workspace"


def _declared_dependency(
    alias: str,
    value: object,
    base: Path,
    workspace_members: Mapping[Path, str],
) -> tuple[str, DependencyContract]:
    table = value if isinstance(value, dict) else None
    if table is not None and "default_features" in table:
        raise ProvenanceError(
            f"dependency {alias!r} uses forbidden default_features spelling"
        )
    if table is not None and "workspace" in table:
        raise ProvenanceError(
            f"dependency {alias!r} may use workspace inheritance only in a member manifest"
        )
    package, requirement = _package_and_requirement(alias, value)
    return package, DependencyContract(
        requirement=requirement,
        resolution_kind=_dependency_resolution_kind(
            alias, package, table, base, workspace_members
        ),
        optional=_dependency_bool(alias, table, "optional", False),
        uses_default_features=_dependency_bool(
            alias, table, "default-features", True
        ),
        features=_dependency_features(alias, table),
    )


def _authored_dependency(
    alias: str,
    value: object,
    workspace_dependencies: Mapping[str, object],
    workspace_root: Path,
    owner_root: Path,
    workspace_members: Mapping[Path, str],
) -> tuple[str, str | None, DependencyContract]:
    table = value if isinstance(value, dict) else None
    if table is not None and table.get("workspace") is True:
        if alias not in workspace_dependencies:
            raise ProvenanceError(
                f"workspace dependency {alias!r} has no [workspace.dependencies] owner"
            )
        forbidden_overrides = sorted(
            {"package", "version", "path"}.intersection(table)
            | FORBIDDEN_DEPENDENCY_SOURCE_KEYS.intersection(table)
        )
        if forbidden_overrides:
            raise ProvenanceError(
                f"workspace dependency {alias!r} overrides source identity locally: "
                f"{forbidden_overrides!r}"
            )
        package, contract = _declared_dependency(
            alias,
            workspace_dependencies[alias],
            workspace_root,
            workspace_members,
        )
        if contract.optional:
            raise ProvenanceError(
                f"[workspace.dependencies] entry {alias!r} cannot be optional"
            )
        local_features = _dependency_features(alias, table)
        contract = DependencyContract(
            requirement=contract.requirement,
            resolution_kind=contract.resolution_kind,
            optional=_dependency_bool(alias, table, "optional", False),
            uses_default_features=contract.uses_default_features
            and _dependency_bool(alias, table, "default-features", True),
            features=tuple(sorted(set(contract.features) | set(local_features))),
        )
    else:
        package, contract = _declared_dependency(
            alias, value, owner_root, workspace_members
        )
    rename = alias if alias != package else None
    return package, rename, contract


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
    workspace_root: Path,
    manifest_path: Path,
    manifest: Mapping[str, object],
    workspace_dependencies: Mapping[str, object],
    workspace_members: Mapping[Path, str],
) -> dict[tuple[str, str | None, str, str | None], DependencyContract]:
    package = _required_table(manifest.get("package"), "workspace package")
    owner = package.get("name")
    if not isinstance(owner, str) or not owner:
        raise ProvenanceError("workspace package name must be a non-empty string")
    records: dict[
        tuple[str, str | None, str, str | None], DependencyContract
    ] = {}

    def collect(container: Mapping[str, object], kind: str, target: str | None) -> None:
        for table_name, table_kind in DEPENDENCY_TABLES:
            if table_kind != kind:
                continue
            for alias, value in _dependency_table(owner, container, table_name).items():
                package_name, rename, contract = _authored_dependency(
                    alias,
                    value,
                    workspace_dependencies,
                    workspace_root,
                    manifest_path.parent,
                    workspace_members,
                )
                key = (package_name, rename, kind, target)
                if key in records:
                    raise ProvenanceError(f"duplicate authored dependency identity: {owner} {key!r}")
                records[key] = contract

    for table_name, kind in DEPENDENCY_TABLES:
        table = _dependency_table(owner, manifest, table_name)
        for alias, value in table.items():
            package_name, rename, contract = _authored_dependency(
                alias,
                value,
                workspace_dependencies,
                workspace_root,
                manifest_path.parent,
                workspace_members,
            )
            key = (package_name, rename, kind, None)
            if key in records:
                raise ProvenanceError(f"duplicate authored dependency identity: {owner} {key!r}")
            records[key] = contract

    targets = manifest.get("target", {})
    targets = _required_table(targets, f"workspace package {owner} target table")
    for target, target_value in targets.items():
        target_table = _required_table(target_value, f"workspace target {owner}/{target}")
        for _table_name, kind in DEPENDENCY_TABLES:
            collect(target_table, kind, target)
    return records


def _authored_feature_policy(
    manifest: Mapping[str, object], owner: str
) -> list[dict[str, object]]:
    table = _required_table(
        manifest.get("features", {}), f"workspace package {owner} [features]"
    )
    rows: list[dict[str, object]] = []
    for name, raw_activations in table.items():
        if not isinstance(name, str) or not name:
            raise ProvenanceError(
                f"workspace package {owner} has an invalid feature name: {name!r}"
            )
        if not isinstance(raw_activations, list) or any(
            not isinstance(activation, str) or not activation
            for activation in raw_activations
        ):
            raise ProvenanceError(
                f"workspace feature {owner}/{name} activations must be non-empty strings"
            )
        rows.append(
            {
                "name": name,
                "activations": sorted(set(raw_activations)),
            }
        )
    rows.sort(key=lambda row: str(row["name"]))
    return rows


def _checked_feature_policy(value: object, owner: str) -> list[dict[str, object]]:
    if not isinstance(value, list):
        raise ProvenanceError(f"dependency policy features must be an array: {owner}")
    rows: list[dict[str, object]] = []
    seen: set[str] = set()
    for raw_row in value:
        row = _required_table(raw_row, f"dependency policy feature for {owner}")
        if set(row) != {"name", "activations"}:
            raise ProvenanceError(
                f"dependency policy feature has unknown or missing fields: {owner}"
            )
        name = row.get("name")
        activations = row.get("activations")
        if (
            not isinstance(name, str)
            or not name
            or name in seen
            or not isinstance(activations, list)
            or any(
                not isinstance(activation, str) or not activation
                for activation in activations
            )
        ):
            raise ProvenanceError(f"dependency policy feature is invalid: {owner}")
        seen.add(name)
        rows.append({"name": name, "activations": sorted(set(activations))})
    rows.sort(key=lambda row: str(row["name"]))
    if rows != value:
        raise ProvenanceError(f"dependency policy features are not canonical: {owner}")
    return rows


def validate_authored_requirements(
    root: Path,
    resolver: object,
    root_profiles: Mapping[str, object],
    workspace_package: Mapping[str, object],
    workspace_dependencies: Mapping[str, object],
    workspace_lints: Mapping[str, object],
    manifests: Sequence[tuple[Path, Mapping[str, object]]],
    workspace_members: Mapping[Path, str],
) -> None:
    policy_path = _require_unredirected_file(root / POLICY_PATH, root, "dependency policy")
    try:
        policy = json.loads(
            policy_path.read_text(encoding="utf-8"),
            object_pairs_hook=_unique_json_object,
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ProvenanceError(f"cannot parse dependency policy {policy_path}: {error}") from error
    if not isinstance(policy, dict) or policy.get("workspaceResolver") != resolver:
        raise ProvenanceError(
            f"workspace resolver does not match dependency policy: {resolver!r}"
        )
    lock_path = _require_unredirected_file(root / "Cargo.lock", root, "root Cargo.lock")
    lock_digest = hashlib.sha256(lock_path.read_bytes()).hexdigest()
    if policy.get("cargoLockSha256") != lock_digest:
        raise ProvenanceError(
            "Cargo.lock digest does not match dependency policy: "
            f"expected {policy.get('cargoLockSha256')!r}, observed {lock_digest!r}"
        )
    if policy.get("rootProfiles") != root_profiles:
        raise ProvenanceError(
            "root profile map does not match dependency policy: "
            f"expected {policy.get('rootProfiles')!r}, observed {root_profiles!r}"
        )
    if policy.get("workspacePackage") != workspace_package:
        raise ProvenanceError(
            "[workspace.package] does not match dependency policy: "
            f"expected {policy.get('workspacePackage')!r}, observed {workspace_package!r}"
        )
    for alias, value in workspace_dependencies.items():
        _, contract = _declared_dependency(alias, value, root, workspace_members)
        if contract.optional:
            raise ProvenanceError(
                f"[workspace.dependencies] entry {alias!r} cannot be optional"
            )
    observed_workspace_dependencies = canonical_workspace_dependency_catalog(
        workspace_dependencies
    )
    if policy.get("workspaceDependencies") != observed_workspace_dependencies:
        raise ProvenanceError("workspace dependency catalog contract drift")
    if policy.get("workspaceLints") != workspace_lints:
        raise ProvenanceError("workspace lint map does not match dependency policy")
    expected_member_lints = _required_table(
        policy.get("workspaceMemberLints"), "dependency policy workspace member lints"
    )
    for owner, lints in expected_member_lints.items():
        _required_table(lints, f"dependency policy lints for {owner}")
    packages = policy.get("packages")
    if not isinstance(packages, list):
        raise ProvenanceError("dependency policy packages must be an array")
    expected: dict[
        str,
        dict[tuple[str, str | None, str, str | None], DependencyContract],
    ] = {}
    expected_packages: dict[str, Mapping[str, object]] = {}
    expected_features: dict[str, list[dict[str, object]]] = {}
    for package in packages:
        package_table = _required_table(package, "dependency policy package")
        owner = package_table.get("name")
        dependencies = package_table.get("dependencies")
        authored_package = package_table.get("authoredPackage")
        if (
            not isinstance(owner, str)
            or not isinstance(dependencies, list)
            or not isinstance(authored_package, dict)
        ):
            raise ProvenanceError(
                "dependency policy package has invalid name, authoredPackage, or dependencies"
            )
        rows: dict[
            tuple[str, str | None, str, str | None], DependencyContract
        ] = {}
        for dependency in dependencies:
            row = _required_table(dependency, f"dependency policy row for {owner}")
            package_name = row.get("package")
            rename = row.get("rename")
            kind = row.get("kind")
            target = row.get("target")
            requirement = row.get("requirement")
            optional = row.get("optional")
            uses_default_features = row.get("usesDefaultFeatures")
            features = row.get("features")
            resolution = row.get("resolution")
            if (
                not isinstance(package_name, str)
                or (rename is not None and not isinstance(rename, str))
                or not isinstance(kind, str)
                or (target is not None and not isinstance(target, str))
                or not isinstance(requirement, str)
                or not isinstance(optional, bool)
                or not isinstance(uses_default_features, bool)
                or not isinstance(features, list)
                or any(not isinstance(feature, str) for feature in features)
                or not isinstance(resolution, dict)
            ):
                raise ProvenanceError(f"dependency policy row has invalid identity: {owner}")
            resolution_kind = resolution.get("kind")
            if resolution.get("package") != package_name or resolution_kind not in {
                "workspace",
                "third-party",
            }:
                raise ProvenanceError(
                    f"dependency policy row has invalid resolution: {owner}/{package_name}"
                )
            if (
                resolution_kind == "third-party"
                and resolution.get("source") != CRATES_IO_SOURCE
            ):
                raise ProvenanceError(
                    f"dependency policy row has unapproved source: {owner}/{package_name}"
                )
            key = (package_name, rename, kind, target)
            if key in rows:
                raise ProvenanceError(f"duplicate dependency policy identity: {owner} {key!r}")
            rows[key] = DependencyContract(
                requirement=requirement,
                resolution_kind=resolution_kind,
                optional=optional,
                uses_default_features=uses_default_features,
                features=tuple(sorted(set(features))),
            )
        if owner in expected:
            raise ProvenanceError(f"duplicate dependency policy package: {owner}")
        expected[owner] = rows
        expected_packages[owner] = authored_package
        expected_features[owner] = _checked_feature_policy(
            package_table.get("features"), owner
        )

    observed: dict[
        str,
        dict[tuple[str, str | None, str, str | None], DependencyContract],
    ] = {}
    observed_packages: dict[str, Mapping[str, object]] = {}
    observed_features: dict[str, list[dict[str, object]]] = {}
    observed_member_lints: dict[str, Mapping[str, object]] = {}
    for manifest_path, manifest in manifests:
        package = _required_table(manifest.get("package"), "workspace package")
        owner = package.get("name")
        if not isinstance(owner, str) or owner in observed:
            raise ProvenanceError(f"invalid or duplicate workspace package name: {owner!r}")
        observed_packages[owner] = package
        observed_features[owner] = _authored_feature_policy(manifest, owner)
        observed_member_lints[owner] = _required_table(
            manifest.get("lints", {}), f"workspace package {owner} [lints]"
        )
        observed[owner] = _authored_requirements(
            root,
            manifest_path,
            manifest,
            workspace_dependencies,
            workspace_members,
        )
    if observed != expected:
        raise ProvenanceError(
            f"authored dependency contract drift: expected {expected!r}, observed {observed!r}"
        )
    if observed_packages != expected_packages:
        raise ProvenanceError(
            "authored workspace package contract drift: "
            f"expected {expected_packages!r}, observed {observed_packages!r}"
        )
    if observed_features != expected_features:
        raise ProvenanceError(
            "authored workspace feature contract drift: "
            f"expected {expected_features!r}, observed {observed_features!r}"
        )
    if observed_member_lints != expected_member_lints:
        raise ProvenanceError(
            "authored workspace lint contract drift: "
            f"expected {expected_member_lints!r}, observed {observed_member_lints!r}"
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
    root_profiles = _required_table(root_manifest.get("profile", {}), "root [profile]")
    workspace_package = _required_table(
        workspace.get("package", {}), "[workspace.package]"
    )
    workspace_dependencies = _required_table(
        workspace.get("dependencies", {}), "[workspace.dependencies]"
    )
    workspace_lints = _required_table(workspace.get("lints", {}), "[workspace.lints]")
    members = workspace.get("members")
    if not isinstance(members, list) or not members:
        raise ProvenanceError("workspace members must be a non-empty explicit list")

    manifests: list[Path] = []
    parsed_manifests: list[tuple[Path, Mapping[str, object]]] = []
    seen: set[Path] = set()
    if root_manifest.get("package") is not None:
        manifests.append(root_manifest_path)
        parsed_manifests.append((root_manifest_path, root_manifest))
        seen.add(root_manifest_path)
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
        parsed_manifests.append((manifest_path, manifest))

    if not manifests:
        raise ProvenanceError("zero workspace member manifests were parsed")
    workspace_members: dict[Path, str] = {}
    for manifest_path, manifest in parsed_manifests:
        package = _required_table(manifest.get("package"), "workspace package")
        package_name = package.get("name")
        if not isinstance(package_name, str) or not package_name:
            raise ProvenanceError(
                f"workspace package name must be a non-empty string: {manifest_path}"
            )
        package_root = manifest_path.parent
        if package_root in workspace_members or package_name in workspace_members.values():
            raise ProvenanceError(f"duplicate workspace package identity: {package_name}")
        workspace_members[package_root] = package_name
    validate_authored_requirements(
        root,
        workspace.get("resolver"),
        root_profiles,
        workspace_package,
        workspace_dependencies,
        workspace_lints,
        parsed_manifests,
        workspace_members,
    )
    return tuple(manifests)


def validate_command(command: Sequence[str]) -> None:
    if not command or command[0] != "cargo":
        raise ProvenanceError("guard command must begin with exact executable name 'cargo'")
    for argument in command[1:]:
        if argument == "--config" or argument.startswith("--config="):
            raise ProvenanceError(f"Cargo global configuration argument is forbidden: {argument}")


def command_may_execute_repository_runtime(command: Sequence[str]) -> bool:
    return any(argument in TERMINAL_CARGO_SUBCOMMANDS for argument in command[1:])


def controlled_child_environment(
    command: Sequence[str],
    environment: Mapping[str, str],
    *,
    cargo: Path | None = None,
    rustc: Path | None = None,
    rustdoc: Path | None = None,
) -> dict[str, str]:
    child = dict(environment)
    if command_may_execute_repository_runtime(command):
        for raw_name in tuple(child):
            if raw_name.upper() in GITHUB_COMMAND_FILE_ENVIRONMENT:
                child.pop(raw_name)
    for name, path in (("CARGO", cargo), ("RUSTC", rustc), ("RUSTDOC", rustdoc)):
        if path is not None:
            child[name] = str(path)
    if cargo is not None:
        child["CARGO_INCREMENTAL"] = "0"
    return child


def _external_executable(
    name: str, root: Path, environment: Mapping[str, str]
) -> Path:
    recorded_name = {"cargo": "LUMIN_CARGO", "rustc": "LUMIN_RUSTC"}.get(name)
    recorded = _environment_value(environment, recorded_name) if recorded_name else None
    if recorded is None and name in {"cargo", "rustc"}:
        rustup = shutil.which("rustup", path=_environment_value(environment, "PATH"))
        if rustup is None:
            raise ProvenanceError(f"rustup cannot resolve exact tool: {name}")
        rustup_lexical = _absolute(Path(rustup))
        try:
            rustup_physical = rustup_lexical.resolve(strict=True)
        except OSError as error:
            raise ProvenanceError(f"cannot resolve rustup executable: {error}") from error
        if (
            rustup_physical != rustup_lexical
            or not rustup_physical.is_file()
            or _is_within(rustup_physical, root)
        ):
            raise ProvenanceError(
                "rustup is redirected, non-file, or repository-owned: "
                f"{rustup_lexical} -> {rustup_physical}"
            )
        completed = subprocess.run(
            [str(rustup_physical), "which", "--toolchain", "1.96.0", name],
            text=True,
            capture_output=True,
            shell=False,
            check=False,
            env=dict(environment),
        )
        if completed.returncode != 0:
            raise ProvenanceError(
                f"rustup cannot resolve exact {name}: {completed.stderr.strip()}"
            )
        resolved = completed.stdout.strip()
    else:
        resolved = recorded or shutil.which(
            name, path=_environment_value(environment, "PATH")
        )
    if resolved is None:
        raise ProvenanceError(f"required executable is unavailable: {name}")
    lexical = _absolute(Path(resolved))
    try:
        physical = lexical.resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"cannot resolve executable {name}: {error}") from error
    if physical != lexical or not physical.is_file() or _is_within(physical, root):
        raise ProvenanceError(
            f"executable is redirected, non-file, or repository-owned: {name} {lexical} -> {physical}"
        )
    return physical


def _toolchain_sibling(reference: Path, name: str, root: Path) -> Path:
    suffix = reference.suffix if os.name == "nt" else ""
    lexical = _absolute(reference.with_name(f"{name}{suffix}"))
    try:
        physical = lexical.resolve(strict=True)
    except OSError as error:
        raise ProvenanceError(f"cannot resolve exact executable {name}: {error}") from error
    if physical != lexical or not physical.is_file() or _is_within(physical, root):
        raise ProvenanceError(
            f"toolchain executable is redirected, non-file, or repository-owned: "
            f"{name} {lexical} -> {physical}"
        )
    return physical


def _verified_helper(root: Path, path: Path, expected_digest: str) -> Path:
    helper = _require_unredirected_file(root / path, root, f"bootstrap helper {path}")
    try:
        observed_digest = hashlib.sha256(helper.read_bytes()).hexdigest()
    except OSError as error:
        raise ProvenanceError(f"cannot hash bootstrap helper {helper}: {error}") from error
    if observed_digest != expected_digest:
        raise ProvenanceError(
            f"bootstrap helper digest mismatch for {path}: "
            f"expected {expected_digest}, observed {observed_digest}"
        )
    return helper


def _parse_unique_json(source: str, context: str) -> object:
    try:
        return json.loads(source, object_pairs_hook=_unique_json_object)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ProvenanceError(f"cannot parse {context}: {error}") from error


def _run_json_helper(
    helper: Path,
    envelope: Mapping[str, object],
    environment: Mapping[str, str],
) -> Mapping[str, object]:
    completed = subprocess.run(
        [sys.executable, "-I", "-S", str(helper)],
        input=json.dumps(envelope, sort_keys=True, separators=(",", ":")),
        text=True,
        capture_output=True,
        shell=False,
        check=False,
        env=dict(environment),
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip() or "no diagnostics"
        raise ProvenanceError(
            f"bootstrap helper failed ({helper.name}, exit {completed.returncode}): {detail}"
        )
    verdict = _parse_unique_json(completed.stdout, f"{helper.name} verdict")
    if not isinstance(verdict, dict):
        raise ProvenanceError(f"bootstrap helper verdict is not an object: {helper.name}")
    return verdict


def _run_capture(
    command: Sequence[str], environment: Mapping[str, str], context: str
) -> str:
    completed = subprocess.run(
        command,
        text=True,
        capture_output=True,
        shell=False,
        check=False,
        env=dict(environment),
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip() or "no diagnostics"
        raise ProvenanceError(
            f"{context} failed (exit {completed.returncode}): {detail}"
        )
    return completed.stdout


def _toolchain_host(
    cargo: Path, rustc: Path, environment: Mapping[str, str]
) -> str:
    cargo_version = _run_capture([str(cargo), "-Vv"], environment, "Cargo identity probe")
    rustc_version = _run_capture([str(rustc), "-vV"], environment, "rustc identity probe")
    if "release: 1.96.0" not in cargo_version or (
        "commit-hash: 30a34c6821b57de0aaec83a901aca39f88f6778c"
        not in cargo_version
    ):
        raise ProvenanceError("Cargo identity is not exact release 1.96.0")
    if "release: 1.96.0" not in rustc_version or (
        "commit-hash: ac68faa20c58cbccd01ee7208bf3b6e93a7d7f96"
        not in rustc_version
    ):
        raise ProvenanceError("rustc identity is not exact release 1.96.0")
    hosts = [
        line.removeprefix("host: ").strip()
        for line in rustc_version.splitlines()
        if line.startswith("host: ")
    ]
    cargo_hosts = [
        line.removeprefix("host: ").strip()
        for line in cargo_version.splitlines()
        if line.startswith("host: ")
    ]
    if len(hosts) != 1 or cargo_hosts != hosts or hosts[0] not in {
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
    }:
        raise ProvenanceError(
            f"Cargo/rustc host mismatch or unsupported host: cargo={cargo_hosts!r}, rustc={hosts!r}"
        )
    return hosts[0]


def _effective_lane(command: Sequence[str], host: str) -> str:
    targets: list[str] = []
    for index, argument in enumerate(command):
        if argument == "--target" and index + 1 < len(command):
            targets.append(command[index + 1])
        elif argument.startswith("--target="):
            targets.append(argument.partition("=")[2])
    if not targets:
        return host
    if targets == ["x86_64-unknown-linux-musl"] and host == "x86_64-unknown-linux-gnu":
        return targets[0]
    raise ProvenanceError(f"unsupported or duplicate Cargo target lane: {targets!r}")


def _run_metadata(
    cargo: Path,
    environment: Mapping[str, str],
    lane: str | None,
) -> Mapping[str, object]:
    command = [
        str(cargo),
        "metadata",
        "--all-features",
        "--locked",
        "--format-version",
        "1",
    ]
    if lane is not None:
        command.extend(["--filter-platform", lane])
    output = _run_capture(command, environment, "Cargo metadata preflight")
    metadata = _parse_unique_json(output, "Cargo metadata preflight output")
    if not isinstance(metadata, dict):
        raise ProvenanceError("Cargo metadata preflight output is not an object")
    return metadata


def _lock_registry_checksums(root: Path) -> dict[tuple[str, str, str], str]:
    lock_path = _require_unredirected_file(root / "Cargo.lock", root, "root Cargo.lock")
    try:
        lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise ProvenanceError(f"cannot parse root Cargo.lock: {error}") from error
    packages = lock.get("package")
    if not isinstance(packages, list):
        raise ProvenanceError("Cargo.lock package surface is not an array")
    rows: dict[tuple[str, str, str], str] = {}
    for raw_package in packages:
        package = _required_table(raw_package, "Cargo.lock package")
        source = package.get("source")
        if source is None:
            continue
        name = package.get("name")
        version = package.get("version")
        checksum = package.get("checksum")
        if (
            source != CRATES_IO_SOURCE
            or not isinstance(name, str)
            or not isinstance(version, str)
            or not isinstance(checksum, str)
            or len(checksum) != 64
            or any(character not in "0123456789abcdef" for character in checksum)
        ):
            raise ProvenanceError(f"Cargo.lock has an unsupported registry row: {package!r}")
        key = (name, version, source)
        if key in rows:
            raise ProvenanceError(f"Cargo.lock has a duplicate registry row: {key!r}")
        rows[key] = checksum
    if not rows:
        raise ProvenanceError("Cargo.lock contains zero crates.io registry rows")
    return rows


def _registry_envelope(
    validation: RepositoryValidation,
    metadata_verdict: Mapping[str, object],
) -> dict[str, object]:
    raw_packages = metadata_verdict.get("registryPackages")
    if not isinstance(raw_packages, list):
        raise ProvenanceError("metadata helper omitted registryPackages")
    checksums = _lock_registry_checksums(validation.root)
    packages: list[dict[str, object]] = []
    observed_keys: set[tuple[str, str, str]] = set()
    for raw_package in raw_packages:
        package = _required_table(raw_package, "metadata registry package")
        name = package.get("name")
        version = package.get("version")
        source = package.get("source")
        manifest_path = package.get("manifestPath")
        if not all(isinstance(value, str) for value in (name, version, source, manifest_path)):
            raise ProvenanceError(f"metadata registry package is malformed: {package!r}")
        key = (name, version, source)
        checksum = checksums.get(key)
        if checksum is None or key in observed_keys:
            raise ProvenanceError(f"metadata/lock registry identity mismatch: {key!r}")
        observed_keys.add(key)
        packages.append(
            {
                "name": name,
                "version": version,
                "source": source,
                "checksum": checksum,
                "manifestPath": manifest_path,
            }
        )
    if observed_keys != set(checksums):
        raise ProvenanceError("metadata registry identities do not equal Cargo.lock registry rows")
    packages.sort(key=lambda package: (str(package["name"]), str(package["version"])))
    return {
        "schemaVersion": 1,
        "repositoryRoot": str(validation.root),
        "cargoHome": str(validation.cargo_home),
        "packages": packages,
    }


def command_requires_dependency_preflight(command: Sequence[str]) -> bool:
    return tuple(command) not in {
        ("cargo", "--version"),
        ("cargo", "-V"),
        ("cargo", "-Vv"),
    }


def run_dependency_preflight(
    validation: RepositoryValidation,
    command: Sequence[str],
    environment: Mapping[str, str],
) -> Path:
    cargo = _external_executable("cargo", validation.root, environment)
    rustc = _external_executable("rustc", validation.root, environment)
    rustdoc = _toolchain_sibling(rustc, "rustdoc", validation.root)
    child_environment = controlled_child_environment(
        command,
        environment,
        cargo=cargo,
        rustc=rustc,
        rustdoc=rustdoc,
    )
    host = _toolchain_host(cargo, rustc, child_environment)
    lane = _effective_lane(command, host)
    unfiltered = _run_metadata(cargo, child_environment, None)
    filtered = _run_metadata(cargo, child_environment, lane)
    target_directory = unfiltered.get("target_directory")
    if not isinstance(target_directory, str):
        raise ProvenanceError("Cargo metadata omitted target_directory")
    metadata_helper = _verified_helper(
        validation.root, METADATA_HELPER_PATH, METADATA_HELPER_SHA256
    )
    metadata_verdict = _run_json_helper(
        metadata_helper,
        {
            "schemaVersion": 1,
            "repositoryRoot": str(validation.root),
            "cargoHome": str(validation.cargo_home),
            "targetDirectory": target_directory,
            "workspaceManifests": [str(path) for path in validation.manifests],
            "policyPath": str(validation.root / POLICY_PATH),
            "effectiveLane": lane,
            "unfiltered": unfiltered,
            "filtered": filtered,
        },
        child_environment,
    )
    registry_helper = _verified_helper(
        validation.root, REGISTRY_HELPER_PATH, REGISTRY_HELPER_SHA256
    )
    registry_verdict = _run_json_helper(
        registry_helper,
        _registry_envelope(validation, metadata_verdict),
        child_environment,
    )
    if registry_verdict != {
        "schemaVersion": 1,
        "packageCount": len(metadata_verdict.get("registryPackages", [])),
    }:
        raise ProvenanceError(f"registry helper returned an invalid verdict: {registry_verdict!r}")
    return cargo


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


def validate_private_output_locations(
    root: Path,
    environment: Mapping[str, str],
    cwd: Path,
    cargo_home: Path,
) -> None:
    target_value = _environment_value(environment, "CARGO_TARGET_DIR")
    target = _absolute(Path(target_value), base=cwd) if target_value else None
    if target is not None and _is_within(target.resolve(strict=False), root):
        raise ProvenanceError(
            f"Cargo target directory must remain outside the repository: {target}"
        )

    if _environment_value(environment, "GITHUB_ACTIONS") != "true":
        return
    runner_value = _environment_value(environment, "RUNNER_TEMP")
    if not runner_value:
        raise ProvenanceError("GitHub Actions requires an exact RUNNER_TEMP")
    runner_temp = _absolute(Path(runner_value), base=cwd)
    physical_runner = runner_temp.resolve(strict=False)
    if physical_runner != runner_temp or _is_within(physical_runner, root):
        raise ProvenanceError(
            f"GitHub runner temp is redirected or repository-owned: {runner_temp}"
        )
    expected_home = runner_temp / "lumin-cargo-home"
    expected_target = runner_temp / "lumin-target"
    if cargo_home != expected_home:
        raise ProvenanceError(
            f"GitHub Cargo home must be job-private {expected_home}, got {cargo_home}"
        )
    if target != expected_target:
        raise ProvenanceError(
            f"GitHub Cargo target must be job-private {expected_target}, got {target}"
        )


def validate_repository(
    root: Path,
    environment: Mapping[str, str],
    cwd: Path,
    cargo_home: Path | None = None,
) -> RepositoryValidation:
    canonical_root = _absolute(root.resolve(strict=True))
    canonical_cwd = _absolute(cwd.resolve(strict=True))
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
    validate_private_output_locations(
        canonical_root,
        environment,
        canonical_cwd,
        lexical_home,
    )
    registry_source = lexical_home / "registry" / "src"
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
    reject_cargo_configuration(canonical_root, lexical_home)
    validate_workflow_surface(canonical_root)
    manifests = validate_workspace_manifests(canonical_root)
    return RepositoryValidation(canonical_root, lexical_home, manifests)


def validate_invocation(
    root: Path,
    command: Sequence[str],
    environment: Mapping[str, str],
    cwd: Path,
    cargo_home: Path | None = None,
) -> RepositoryValidation:
    validate_command(command)
    return validate_repository(root, environment, cwd, cargo_home)


def _parse_command(arguments: Sequence[str]) -> tuple[str, ...]:
    if not arguments or arguments[0] != "--" or len(arguments) == 1:
        raise ProvenanceError("usage: source_provenance.py -- cargo <arguments>")
    return tuple(arguments[1:])


def validate_check_only(
    root: Path,
    environment: Mapping[str, str],
    cwd: Path,
    preflight: Callable[
        [RepositoryValidation, Sequence[str], Mapping[str, str]], Path
    ] = run_dependency_preflight,
    cargo_home: Path | None = None,
) -> None:
    validation = validate_repository(root, environment, cwd, cargo_home)
    preflight(validation, ("cargo", "metadata"), environment)


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        root = repository_root()
        ensure_runtime(root)
        raw_arguments = tuple(sys.argv[1:] if arguments is None else arguments)
        if raw_arguments == ("--check-only",):
            validate_check_only(root, os.environ, Path.cwd())
            return 0
        command = _parse_command(raw_arguments)
        validation = validate_invocation(root, command, os.environ, Path.cwd())
        cargo = _external_executable("cargo", validation.root, os.environ)
        if command_requires_dependency_preflight(command):
            cargo = run_dependency_preflight(validation, command, os.environ)
            rustc = _external_executable("rustc", validation.root, os.environ)
            rustdoc = _toolchain_sibling(rustc, "rustdoc", validation.root)
        else:
            rustc = None
            rustdoc = None
        resolved_command = (str(cargo), *command[1:])
        completed = subprocess.run(
            resolved_command,
            shell=False,
            check=False,
            env=controlled_child_environment(
                command,
                os.environ,
                cargo=cargo,
                rustc=rustc,
                rustdoc=rustdoc,
            ),
        )
        return completed.returncode if completed.returncode >= 0 else 128 - completed.returncode
    except ProvenanceError as error:
        print(f"[source-provenance] {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
