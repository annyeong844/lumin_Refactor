"""Compare Cargo metadata with the frozen dependency-surface policy."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import sys
from collections.abc import Mapping, Sequence


MINIMUM_PYTHON = (3, 11)
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
ROOT_FIELDS = frozenset(
    {
        "schemaVersion",
        "repositoryRoot",
        "cargoHome",
        "targetDirectory",
        "workspaceManifests",
        "policyPath",
        "effectiveLane",
        "unfiltered",
        "filtered",
    }
)
POLICY_FIELDS = frozenset(
    {
        "schemaVersion",
        "workspaceResolver",
        "cargoLockSha256",
        "rootProfiles",
        "workspaceDependencies",
        "workspaceLints",
        "workspaceMemberLints",
        "workspacePackage",
        "packages",
        "packageDefinitions",
        "resolvedGraph",
        "resolutionLaneDigests",
    }
)
SUPPORTED_LANES = frozenset(
    {
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
        "x86_64-unknown-linux-musl",
    }
)


class SnapshotError(RuntimeError):
    """Cargo metadata cannot authorize compilation."""


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            raise SnapshotError(f"duplicate JSON key: {key!r}")
        value[key] = item
    return value


def _object(value: object, context: str) -> Mapping[str, object]:
    if not isinstance(value, dict):
        raise SnapshotError(f"{context} must be an object")
    return value


def _exact_object(
    value: object, fields: frozenset[str], context: str
) -> Mapping[str, object]:
    result = _object(value, context)
    if set(result) != fields:
        raise SnapshotError(
            f"{context} fields differ: expected {sorted(fields)!r}, got {sorted(result)!r}"
        )
    return result


def _field(value: Mapping[str, object], name: str, context: str) -> object:
    if name not in value:
        raise SnapshotError(f"{context} is missing {name}")
    return value[name]


def _string(value: object, context: str) -> str:
    if not isinstance(value, str) or not value:
        raise SnapshotError(f"{context} must be a non-empty string")
    return value


def _optional_string(value: object, context: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise SnapshotError(f"{context} must be null or a string")
    return value


def _boolean(value: object, context: str) -> bool:
    if not isinstance(value, bool):
        raise SnapshotError(f"{context} must be a boolean")
    return value


def _array(value: object, context: str) -> list[object]:
    if not isinstance(value, list):
        raise SnapshotError(f"{context} must be an array")
    return value


def _canonical_strings(value: object, context: str) -> list[str]:
    values = _array(value, context)
    if any(not isinstance(item, str) for item in values):
        raise SnapshotError(f"{context} contains a non-string")
    return sorted(set(values))


def _dependency_kind(value: object, context: str) -> str:
    if value is None:
        return "normal"
    if value in {"dev", "build"}:
        return str(value)
    raise SnapshotError(f"{context} has an unknown dependency kind: {value!r}")


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


def _stable_workspace_path(path: str, repository_root: Path, context: str) -> str:
    lexical = _absolute(Path(path))
    try:
        physical = lexical.resolve(strict=True)
    except OSError as error:
        raise SnapshotError(f"cannot resolve {context} {lexical}: {error}") from error
    if physical != lexical or not _is_within(physical, repository_root):
        raise SnapshotError(f"{context} escapes or redirects from the repository: {lexical}")
    return physical.relative_to(repository_root).as_posix()


def _json_key(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def _sort_unique(values: list[object], context: str) -> list[object]:
    keyed = [(_json_key(value), value) for value in values]
    keyed.sort(key=lambda pair: pair[0])
    for left, right in zip(keyed, keyed[1:]):
        if left[0] == right[0]:
            raise SnapshotError(f"duplicate {context}: {left[0]}")
    return [value for _, value in keyed]


def ensure_runtime(repository_root: Path) -> None:
    if sys.version_info < MINIMUM_PYTHON:
        raise SnapshotError("Python 3.11 or newer is required")
    if sys.flags.isolated != 1 or sys.flags.no_site != 1 or not sys.flags.safe_path:
        raise SnapshotError("invoke metadata_snapshot.py with Python -I -S")
    for entry in sys.path:
        if not entry:
            raise SnapshotError("empty Python import path is forbidden")
        if _is_within(Path(entry).resolve(strict=False), repository_root):
            raise SnapshotError(f"repository Python import path is forbidden: {entry}")


def _feature_policy(package: Mapping[str, object], owner: str) -> list[object]:
    features = _object(_field(package, "features", f"package {owner}"), f"features for {owner}")
    rows: list[object] = []
    for name, activations in features.items():
        if not name:
            raise SnapshotError(f"package {owner} has an empty feature name")
        rows.append(
            {
                "name": name,
                "activations": _canonical_strings(
                    activations, f"feature activations for {owner}/{name}"
                ),
            }
        )
    rows.sort(key=lambda row: str(_object(row, "feature row")["name"]))
    return rows


def _stable_identities(
    packages: list[object],
    member_ids: set[str],
    repository_root: Path,
) -> dict[str, object]:
    identities: dict[str, object] = {}
    stable_keys: set[str] = set()
    for raw_package in packages:
        package = _object(raw_package, "metadata package")
        package_id = _string(_field(package, "id", "metadata package"), "package id")
        name = _string(_field(package, "name", "metadata package"), "package name")
        version = _string(_field(package, "version", "metadata package"), "package version")
        if package_id in member_ids:
            identity: object = {
                "kind": "workspace",
                "name": name,
                "version": version,
                "manifest": _stable_workspace_path(
                    _string(
                        _field(package, "manifest_path", "workspace package"),
                        f"manifest path for {name}",
                    ),
                    repository_root,
                    f"workspace manifest for {name}",
                ),
            }
        else:
            source = _string(
                _field(package, "source", "registry package"),
                f"source for {name} {version}",
            )
            if source != CRATES_IO_SOURCE:
                raise SnapshotError(f"unapproved package source: {name} {version} {source}")
            identity = {
                "kind": "registry",
                "name": name,
                "version": version,
                "source": source,
            }
        key = _json_key(identity)
        if package_id in identities or key in stable_keys:
            raise SnapshotError(f"duplicate package identity: {package_id}")
        identities[package_id] = identity
        stable_keys.add(key)
    return identities


def _package_dependency(raw: object, owner: str) -> object:
    dependency = _object(raw, f"dependency definition for {owner}")
    return {
        "name": _string(_field(dependency, "name", "dependency"), "dependency name"),
        "rename": _optional_string(_field(dependency, "rename", "dependency"), "rename"),
        "requirement": _string(
            _field(dependency, "req", "dependency"), "dependency requirement"
        ),
        "source": _field(dependency, "source", "dependency"),
        "registry": _field(dependency, "registry", "dependency"),
        "kind": _dependency_kind(_field(dependency, "kind", "dependency"), "dependency"),
        "target": _optional_string(_field(dependency, "target", "dependency"), "target"),
        "optional": _boolean(_field(dependency, "optional", "dependency"), "optional"),
        "usesDefaultFeatures": _boolean(
            _field(dependency, "uses_default_features", "dependency"),
            "uses_default_features",
        ),
        "features": _canonical_strings(
            _field(dependency, "features", "dependency"), "dependency features"
        ),
    }


def _registry_package_root(
    package: Mapping[str, object], cargo_home: Path, owner: str
) -> Path:
    manifest = _require_unredirected(
        Path(
            _string(
                _field(package, "manifest_path", "registry package"),
                f"manifest path for {owner}",
            )
        ),
        "file",
        f"registry manifest for {owner}",
    )
    if manifest.name != "Cargo.toml":
        raise SnapshotError(f"registry manifest has unexpected name: {manifest}")
    package_root = _require_unredirected(
        manifest.parent, "directory", f"registry package root for {owner}"
    )
    registry_root = _require_unredirected(
        cargo_home / "registry" / "src", "directory", "Cargo registry source root"
    )
    if not _is_within(package_root, registry_root):
        raise SnapshotError(f"registry package escapes Cargo home: {package_root}")
    return package_root


def _package_target(
    package: Mapping[str, object],
    raw_target: object,
    is_workspace: bool,
    repository_root: Path,
    cargo_home: Path,
    owner: str,
) -> object:
    target = _object(raw_target, f"target for {owner}")
    raw_source = _string(_field(target, "src_path", "target"), f"source for {owner}")
    if is_workspace:
        source = _stable_workspace_path(
            raw_source, repository_root, f"workspace target for {owner}"
        )
    else:
        package_root = _registry_package_root(package, cargo_home, owner)
        source_path = _require_unredirected(
            Path(raw_source), "file", f"registry target source for {owner}"
        )
        if not _is_within(source_path, package_root):
            raise SnapshotError(f"registry target escapes package {owner}: {source_path}")
        source = source_path.relative_to(package_root).as_posix()
    required_features = target.get("required-features", [])
    return {
        "name": _string(_field(target, "name", "target"), "target name"),
        "edition": _string(_field(target, "edition", "target"), "target edition"),
        "doc": _boolean(_field(target, "doc", "target"), "target doc"),
        "doctest": _boolean(_field(target, "doctest", "target"), "target doctest"),
        "test": _boolean(_field(target, "test", "target"), "target test"),
        "kind": _canonical_strings(_field(target, "kind", "target"), "target kind"),
        "crateTypes": _canonical_strings(
            _field(target, "crate_types", "target"), "target crate types"
        ),
        "requiredFeatures": _canonical_strings(
            required_features, "target required features"
        ),
        "source": source,
    }


def _package_definitions(
    packages: list[object],
    member_ids: set[str],
    identities: Mapping[str, object],
    repository_root: Path,
    cargo_home: Path,
) -> list[object]:
    definitions: list[object] = []
    for raw_package in packages:
        package = _object(raw_package, "metadata package")
        package_id = _string(_field(package, "id", "metadata package"), "package id")
        owner = _string(_field(package, "name", "metadata package"), "package name")
        dependencies = [
            _package_dependency(dependency, owner)
            for dependency in _array(
                _field(package, "dependencies", "metadata package"),
                f"dependencies for {owner}",
            )
        ]
        targets = [
            _package_target(
                package,
                target,
                package_id in member_ids,
                repository_root,
                cargo_home,
                owner,
            )
            for target in _array(
                _field(package, "targets", "metadata package"), f"targets for {owner}"
            )
        ]
        definitions.append(
            {
                "identity": identities[package_id],
                "links": _field(package, "links", "metadata package"),
                "rustVersion": _field(package, "rust_version", "metadata package"),
                "features": _feature_policy(package, owner),
                "dependencies": _sort_unique(
                    dependencies, f"package dependency definition for {owner}"
                ),
                "targets": _sort_unique(targets, f"package target definition for {owner}"),
            }
        )
    return _sort_unique(definitions, "package definition")


def _resolved_graph(
    metadata: Mapping[str, object],
    identities: Mapping[str, object],
) -> object:
    resolve = _object(_field(metadata, "resolve", "metadata"), "metadata resolve")
    root_id = _field(resolve, "root", "metadata resolve")
    if root_id is None:
        root: object = None
    elif isinstance(root_id, str) and root_id in identities:
        root = identities[root_id]
    else:
        raise SnapshotError(f"metadata resolve root is invalid: {root_id!r}")
    nodes: list[object] = []
    seen_ids: set[str] = set()
    for raw_node in _array(_field(resolve, "nodes", "metadata resolve"), "resolve nodes"):
        node = _object(raw_node, "resolve node")
        node_id = _string(_field(node, "id", "resolve node"), "resolve node id")
        if node_id in seen_ids or node_id not in identities:
            raise SnapshotError(f"duplicate or unknown resolve node: {node_id}")
        seen_ids.add(node_id)
        dependencies: list[object] = []
        dependency_keys: set[str] = set()
        for dependency_id in _array(
            _field(node, "dependencies", "resolve node"), "resolve dependencies"
        ):
            if not isinstance(dependency_id, str) or dependency_id not in identities:
                raise SnapshotError(f"unknown resolve dependency: {dependency_id!r}")
            identity = identities[dependency_id]
            key = _json_key(identity)
            if key in dependency_keys:
                raise SnapshotError(f"duplicate resolve dependency: {dependency_id}")
            dependency_keys.add(key)
            dependencies.append(identity)
        bindings: list[object] = []
        binding_keys: set[str] = set()
        for raw_binding in _array(_field(node, "deps", "resolve node"), "resolve bindings"):
            binding = _object(raw_binding, "resolve binding")
            target_id = _string(_field(binding, "pkg", "resolve binding"), "binding package")
            if target_id not in identities:
                raise SnapshotError(f"unknown binding package: {target_id}")
            target = identities[target_id]
            binding_keys.add(_json_key(target))
            kinds: list[object] = []
            for raw_kind in _array(
                _field(binding, "dep_kinds", "resolve binding"), "binding kinds"
            ):
                kind = _object(raw_kind, "binding kind")
                kinds.append(
                    {
                        "kind": _dependency_kind(
                            _field(kind, "kind", "binding kind"), "binding kind"
                        ),
                        "target": _optional_string(
                            _field(kind, "target", "binding kind"), "binding target"
                        ),
                    }
                )
            if not kinds:
                raise SnapshotError(f"resolved dependency has zero kinds: {target_id}")
            bindings.append(
                {
                    "binding": _string(
                        _field(binding, "name", "resolve binding"), "binding name"
                    ),
                    "package": target,
                    "kinds": _sort_unique(kinds, "resolved dependency kind"),
                }
            )
        if dependency_keys != binding_keys:
            raise SnapshotError(f"resolve dependencies and bindings disagree: {node_id}")
        nodes.append(
            {
                "package": identities[node_id],
                "features": _canonical_strings(
                    _field(node, "features", "resolve node"), "resolve node features"
                ),
                "dependencies": _sort_unique(dependencies, "resolved dependency"),
                "bindings": _sort_unique(bindings, "resolved dependency binding"),
            }
        )
    if seen_ids != set(identities):
        raise SnapshotError("resolve node ids do not equal the complete package id set")
    return {"root": root, "nodes": _sort_unique(nodes, "resolve node")}


def _metadata_projection(
    raw_metadata: object,
    repository_root: Path,
    cargo_home: Path,
    target_directory: Path,
) -> tuple[list[object], object, list[dict[str, str]], set[str]]:
    metadata = _object(raw_metadata, "Cargo metadata")
    for field_name in ("target_directory", "build_directory"):
        observed = _absolute(
            Path(
                _string(
                    _field(metadata, field_name, "Cargo metadata"),
                    f"metadata {field_name}",
                )
            )
        )
        if observed != target_directory or observed.resolve(strict=False) != target_directory:
            raise SnapshotError(
                f"metadata {field_name} differs from the admitted target: {observed}"
            )
    packages = _array(_field(metadata, "packages", "Cargo metadata"), "metadata packages")
    raw_members = _array(
        _field(metadata, "workspace_members", "Cargo metadata"), "workspace members"
    )
    if any(not isinstance(member, str) for member in raw_members):
        raise SnapshotError("workspace member ids must be strings")
    member_ids = set(raw_members)
    if len(member_ids) != len(raw_members):
        raise SnapshotError("duplicate workspace member id")
    identities = _stable_identities(packages, member_ids, repository_root)
    if member_ids - set(identities):
        raise SnapshotError("workspace member package is absent from metadata packages")
    definitions = _package_definitions(
        packages, member_ids, identities, repository_root, cargo_home
    )
    graph = _resolved_graph(metadata, identities)
    registry_packages: list[dict[str, str]] = []
    workspace_manifests: set[str] = set()
    for raw_package in packages:
        package = _object(raw_package, "metadata package")
        package_id = _string(_field(package, "id", "metadata package"), "package id")
        manifest = _string(
            _field(package, "manifest_path", "metadata package"), "manifest path"
        )
        if package_id in member_ids:
            workspace_manifests.add(
                str(_require_unredirected(Path(manifest), "file", "workspace manifest"))
            )
            continue
        identity = _object(identities[package_id], "registry identity")
        registry_packages.append(
            {
                "name": _string(identity.get("name"), "registry name"),
                "version": _string(identity.get("version"), "registry version"),
                "source": _string(identity.get("source"), "registry source"),
                "manifestPath": str(
                    _require_unredirected(Path(manifest), "file", "registry manifest")
                ),
            }
        )
    registry_packages.sort(key=lambda package: (package["name"], package["version"]))
    if len({(row["name"], row["version"]) for row in registry_packages}) != len(
        registry_packages
    ):
        raise SnapshotError("duplicate registry package name/version identity")
    return definitions, graph, registry_packages, workspace_manifests


def _load_policy(path: Path, repository_root: Path) -> Mapping[str, object]:
    policy_path = _require_unredirected(path, "file", "dependency policy")
    if not _is_within(policy_path, repository_root):
        raise SnapshotError(f"dependency policy is outside the repository: {policy_path}")
    try:
        policy = json.loads(
            policy_path.read_text(encoding="utf-8"), object_pairs_hook=_unique_object
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise SnapshotError(f"cannot parse dependency policy {policy_path}: {error}") from error
    result = _exact_object(policy, POLICY_FIELDS, "dependency policy")
    if result["schemaVersion"] != 1:
        raise SnapshotError("dependency policy schemaVersion must be 1")
    return result


def _resolution_lane_digests(policy: Mapping[str, object]) -> Mapping[str, str]:
    raw = _object(policy["resolutionLaneDigests"], "resolution lane digests")
    if set(raw) != SUPPORTED_LANES:
        raise SnapshotError(
            "resolution lane digest keys differ: "
            f"expected {sorted(SUPPORTED_LANES)!r}, got {sorted(raw)!r}"
        )
    digests: dict[str, str] = {}
    for lane, value in raw.items():
        if (
            not isinstance(value, str)
            or len(value) != 64
            or any(character not in "0123456789abcdef" for character in value)
        ):
            raise SnapshotError(f"resolution lane digest is invalid: {lane!r}")
        digests[lane] = value
    return digests


def validate_snapshot(envelope: object) -> dict[str, object]:
    root = _exact_object(envelope, ROOT_FIELDS, "metadata snapshot envelope")
    if root["schemaVersion"] != 1:
        raise SnapshotError("metadata snapshot schemaVersion must be 1")
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
    target_directory = _absolute(
        Path(_string(root["targetDirectory"], "target directory"))
    )
    if target_directory.resolve(strict=False) != target_directory:
        raise SnapshotError(f"target directory is redirected: {target_directory}")
    lane = _string(root["effectiveLane"], "effective resolution lane")
    if lane not in SUPPORTED_LANES:
        raise SnapshotError(f"unsupported resolution lane: {lane}")
    expected_manifests_raw = _array(root["workspaceManifests"], "workspace manifests")
    expected_manifests = {
        str(_require_unredirected(Path(_string(path, "workspace manifest")), "file", "workspace manifest"))
        for path in expected_manifests_raw
    }
    if len(expected_manifests) != len(expected_manifests_raw) or not expected_manifests:
        raise SnapshotError("workspace manifest set is empty or duplicate")
    policy = _load_policy(
        Path(_string(root["policyPath"], "dependency policy path")), repository_root
    )
    definitions, graph, registry_packages, manifests = _metadata_projection(
        root["unfiltered"], repository_root, cargo_home, target_directory
    )
    if manifests != expected_manifests:
        raise SnapshotError(
            f"metadata workspace manifests differ: expected {sorted(expected_manifests)!r}, "
            f"got {sorted(manifests)!r}"
        )
    if policy["packageDefinitions"] != definitions:
        raise SnapshotError("unfiltered package definition snapshot differs from policy")
    if policy["resolvedGraph"] != graph:
        raise SnapshotError("unfiltered resolved graph snapshot differs from policy")
    filtered_definitions, filtered_graph, filtered_registry, filtered_manifests = (
        _metadata_projection(root["filtered"], repository_root, cargo_home, target_directory)
    )
    unfiltered_by_identity = {
        _json_key(_object(definition, "package definition")["identity"]): definition
        for definition in definitions
    }
    for definition in filtered_definitions:
        identity = _json_key(_object(definition, "filtered package definition")["identity"])
        if unfiltered_by_identity.get(identity) != definition:
            raise SnapshotError("filtered metadata package surface differs from unfiltered metadata")
    unfiltered_registry_keys = {
        (package["name"], package["version"], package["source"])
        for package in registry_packages
    }
    filtered_registry_keys = {
        (package["name"], package["version"], package["source"])
        for package in filtered_registry
    }
    if not filtered_registry_keys.issubset(unfiltered_registry_keys):
        raise SnapshotError("filtered metadata registry surface is not an unfiltered subset")
    if filtered_manifests != manifests:
        raise SnapshotError("filtered metadata workspace manifests differ")
    filtered_graph_digest = hashlib.sha256(
        _json_key(filtered_graph).encode("utf-8")
    ).hexdigest()
    expected_lane_digest = _resolution_lane_digests(policy)[lane]
    if filtered_graph_digest != expected_lane_digest:
        raise SnapshotError(
            f"filtered resolution graph differs for {lane}: "
            f"expected {expected_lane_digest}, got {filtered_graph_digest}"
        )
    return {
        "schemaVersion": 1,
        "effectiveLane": lane,
        "filteredGraphSha256": filtered_graph_digest,
        "registryPackages": registry_packages,
    }


def _read_stdin() -> object:
    try:
        return json.loads(sys.stdin.read(), object_pairs_hook=_unique_object)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise SnapshotError(f"cannot parse metadata snapshot envelope: {error}") from error


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        if tuple(sys.argv[1:] if arguments is None else arguments):
            raise SnapshotError("metadata_snapshot.py accepts only a stdin envelope")
        verdict = validate_snapshot(_read_stdin())
        print(json.dumps(verdict, sort_keys=True, separators=(",", ":")))
        return 0
    except SnapshotError as error:
        print(f"[metadata-snapshot] {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
