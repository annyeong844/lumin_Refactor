"""Tests for the isolated Cargo metadata policy verifier."""

from __future__ import annotations

import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


sys.dont_write_bytecode = True
SCRIPT = Path(__file__).with_name("metadata_snapshot.py").resolve()
SPEC = importlib.util.spec_from_file_location("lumin_metadata_snapshot", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
SNAPSHOT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SNAPSHOT
SPEC.loader.exec_module(SNAPSHOT)


class Fixture:
    workspace_id = "path+file:///fixture/member#0.1.0"
    registry_id = "registry+https://github.com/rust-lang/crates.io-index#dep@1.0.0"

    def __init__(self, temporary: Path) -> None:
        self.repository = temporary / "repository"
        self.member = self.repository / "member"
        self.cargo_home = temporary / "cargo-home"
        self.target = temporary / "target"
        self.policy = self.repository / "tools" / "xtask" / "dependency-policy.json"
        self.registry_package = (
            self.cargo_home
            / "registry"
            / "src"
            / "index.crates.io-test"
            / "dep-1.0.0"
        )
        (self.member / "src").mkdir(parents=True)
        (self.registry_package / "src").mkdir(parents=True)
        self.target.mkdir()
        self.policy.parent.mkdir(parents=True)
        (self.repository / "Cargo.toml").write_text(
            '[workspace]\nresolver = "3"\nmembers = ["member"]\n', encoding="utf-8"
        )
        (self.member / "Cargo.toml").write_text(
            '[package]\nname = "member"\nversion = "0.1.0"\n', encoding="utf-8"
        )
        (self.member / "src" / "lib.rs").write_text("pub fn member() {}\n", encoding="utf-8")
        (self.registry_package / "Cargo.toml").write_text(
            '[package]\nname = "dep"\nversion = "1.0.0"\n', encoding="utf-8"
        )
        (self.registry_package / "src" / "lib.rs").write_text(
            "pub fn dep() {}\n", encoding="utf-8"
        )
        self.metadata = self.metadata_value()
        self.write_policy()

    def metadata_value(self) -> dict[str, object]:
        workspace_dependency = {
            "name": "dep",
            "source": SNAPSHOT.CRATES_IO_SOURCE,
            "req": "=1.0.0",
            "kind": None,
            "rename": None,
            "optional": False,
            "uses_default_features": True,
            "features": ["std"],
            "target": None,
            "registry": None,
        }
        workspace_target = {
            "kind": ["lib"],
            "crate_types": ["lib"],
            "name": "member",
            "src_path": str(self.member / "src" / "lib.rs"),
            "edition": "2024",
            "doc": True,
            "doctest": True,
            "test": True,
            "required-features": [],
        }
        registry_target = {
            "kind": ["lib"],
            "crate_types": ["lib"],
            "name": "dep",
            "src_path": str(self.registry_package / "src" / "lib.rs"),
            "edition": "2021",
            "doc": True,
            "doctest": True,
            "test": True,
            "required-features": [],
        }
        return {
            "packages": [
                {
                    "name": "member",
                    "version": "0.1.0",
                    "id": self.workspace_id,
                    "license": None,
                    "license_file": None,
                    "description": None,
                    "source": None,
                    "dependencies": [workspace_dependency],
                    "targets": [workspace_target],
                    "features": {"default": []},
                    "manifest_path": str(self.member / "Cargo.toml"),
                    "metadata": {},
                    "publish": None,
                    "authors": [],
                    "categories": [],
                    "keywords": [],
                    "readme": None,
                    "repository": None,
                    "homepage": None,
                    "documentation": None,
                    "edition": "2024",
                    "links": None,
                    "default_run": None,
                    "rust_version": "1.96",
                },
                {
                    "name": "dep",
                    "version": "1.0.0",
                    "id": self.registry_id,
                    "license": "MIT",
                    "license_file": None,
                    "description": None,
                    "source": SNAPSHOT.CRATES_IO_SOURCE,
                    "dependencies": [],
                    "targets": [registry_target],
                    "features": {"default": [], "std": []},
                    "manifest_path": str(self.registry_package / "Cargo.toml"),
                    "metadata": {},
                    "publish": None,
                    "authors": [],
                    "categories": [],
                    "keywords": [],
                    "readme": None,
                    "repository": None,
                    "homepage": None,
                    "documentation": None,
                    "edition": "2021",
                    "links": None,
                    "default_run": None,
                    "rust_version": "1.60",
                },
            ],
            "workspace_members": [self.workspace_id],
            "workspace_default_members": [self.workspace_id],
            "resolve": {
                "nodes": [
                    {
                        "id": self.workspace_id,
                        "dependencies": [self.registry_id],
                        "deps": [
                            {
                                "name": "dep",
                                "pkg": self.registry_id,
                                "dep_kinds": [{"kind": None, "target": None}],
                            }
                        ],
                        "features": ["default"],
                    },
                    {
                        "id": self.registry_id,
                        "dependencies": [],
                        "deps": [],
                        "features": ["default", "std"],
                    },
                ],
                "root": None,
            },
            "target_directory": str(self.target),
            "build_directory": str(self.target),
            "version": 1,
            "workspace_root": str(self.repository),
            "metadata": None,
        }

    @staticmethod
    def registry_identity() -> dict[str, object]:
        return {
            "kind": "registry",
            "name": "dep",
            "version": "1.0.0",
            "source": SNAPSHOT.CRATES_IO_SOURCE,
        }

    @staticmethod
    def workspace_identity() -> dict[str, object]:
        return {
            "kind": "workspace",
            "name": "member",
            "version": "0.1.0",
            "manifest": "member/Cargo.toml",
        }

    def package_definitions(self) -> list[object]:
        registry = self.registry_identity()
        workspace = self.workspace_identity()
        return [
            {
                "identity": registry,
                "links": None,
                "rustVersion": "1.60",
                "features": [
                    {"name": "default", "activations": []},
                    {"name": "std", "activations": []},
                ],
                "dependencies": [],
                "targets": [
                    {
                        "name": "dep",
                        "edition": "2021",
                        "doc": True,
                        "doctest": True,
                        "test": True,
                        "kind": ["lib"],
                        "crateTypes": ["lib"],
                        "requiredFeatures": [],
                        "source": "src/lib.rs",
                    }
                ],
            },
            {
                "identity": workspace,
                "links": None,
                "rustVersion": "1.96",
                "features": [{"name": "default", "activations": []}],
                "dependencies": [
                    {
                        "name": "dep",
                        "rename": None,
                        "requirement": "=1.0.0",
                        "source": SNAPSHOT.CRATES_IO_SOURCE,
                        "registry": None,
                        "kind": "normal",
                        "target": None,
                        "optional": False,
                        "usesDefaultFeatures": True,
                        "features": ["std"],
                    }
                ],
                "targets": [
                    {
                        "name": "member",
                        "edition": "2024",
                        "doc": True,
                        "doctest": True,
                        "test": True,
                        "kind": ["lib"],
                        "crateTypes": ["lib"],
                        "requiredFeatures": [],
                        "source": "member/src/lib.rs",
                    }
                ],
            },
        ]

    def resolved_graph(self) -> dict[str, object]:
        registry = self.registry_identity()
        workspace = self.workspace_identity()
        return {
            "root": None,
            "nodes": [
                {
                    "package": registry,
                    "features": ["default", "std"],
                    "dependencies": [],
                    "bindings": [],
                },
                {
                    "package": workspace,
                    "features": ["default"],
                    "dependencies": [registry],
                    "bindings": [
                        {
                            "binding": "dep",
                            "package": registry,
                            "kinds": [{"kind": "normal", "target": None}],
                        }
                    ],
                },
            ],
        }

    def write_policy(self) -> None:
        self.policy.write_text(
            json.dumps(
                {
                    "schemaVersion": 1,
                    "workspaceResolver": "3",
                    "cargoLockSha256": "lock",
                    "rootProfiles": {},
                    "workspaceDependencies": {},
                    "workspaceLints": {},
                    "workspaceMemberLints": {"member": {}},
                    "workspacePackage": {},
                    "packages": [],
                    "packageDefinitions": self.package_definitions(),
                    "resolvedGraph": self.resolved_graph(),
                },
                sort_keys=True,
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )

    def envelope(self) -> dict[str, object]:
        return {
            "schemaVersion": 1,
            "repositoryRoot": str(self.repository),
            "cargoHome": str(self.cargo_home),
            "targetDirectory": str(self.target),
            "workspaceManifests": [str(self.member / "Cargo.toml")],
            "policyPath": str(self.policy),
            "effectiveLane": "x86_64-pc-windows-msvc",
            "unfiltered": copy.deepcopy(self.metadata),
            "filtered": copy.deepcopy(self.metadata),
        }


class MetadataSnapshotTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], Fixture]:
        temporary = tempfile.TemporaryDirectory()
        return temporary, Fixture(Path(temporary.name))

    def test_complete_metadata_matches_independent_policy(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            verdict = SNAPSHOT.validate_snapshot(fixture.envelope())
            self.assertEqual(verdict["schemaVersion"], 1)
            self.assertEqual(verdict["effectiveLane"], "x86_64-pc-windows-msvc")
            self.assertEqual(
                verdict["registryPackages"],
                [
                    {
                        "name": "dep",
                        "version": "1.0.0",
                        "source": SNAPSHOT.CRATES_IO_SOURCE,
                        "manifestPath": str(fixture.registry_package / "Cargo.toml"),
                    }
                ],
            )

    def test_transitive_definition_and_resolution_drift_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            definition_drift = fixture.envelope()
            definition_drift["unfiltered"]["packages"][1]["features"]["extra"] = []
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "package definition"):
                SNAPSHOT.validate_snapshot(definition_drift)

            graph_drift = fixture.envelope()
            graph_drift["unfiltered"]["resolve"]["nodes"][1]["features"].append("extra")
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "resolved graph"):
                SNAPSHOT.validate_snapshot(graph_drift)

    def test_set_and_traversal_order_do_not_change_the_verdict(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            envelope = fixture.envelope()
            for lane in (envelope["unfiltered"], envelope["filtered"]):
                lane["packages"].reverse()
                lane["resolve"]["nodes"].reverse()
                lane["resolve"]["nodes"][0]["features"].reverse()
            SNAPSHOT.validate_snapshot(envelope)

    def test_filtered_package_or_workspace_drift_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            envelope = fixture.envelope()
            envelope["filtered"]["packages"][1]["rust_version"] = "9.9"
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "filtered metadata package"):
                SNAPSHOT.validate_snapshot(envelope)

    def test_duplicate_policy_keys_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            source = fixture.policy.read_text(encoding="utf-8").replace(
                '"schemaVersion": 1', '"schemaVersion": 1, "schemaVersion": 2', 1
            )
            fixture.policy.write_text(source, encoding="utf-8")
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "duplicate JSON key"):
                SNAPSHOT.validate_snapshot(fixture.envelope())

    def test_isolated_cli_emits_registry_targets(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            completed = subprocess.run(
                [sys.executable, "-I", "-S", str(SCRIPT)],
                input=json.dumps(fixture.envelope()),
                text=True,
                capture_output=True,
                check=False,
                cwd=fixture.repository,
                env={"SYSTEMROOT": os.environ.get("SYSTEMROOT", "")},
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            verdict = json.loads(completed.stdout)
            self.assertEqual(len(verdict["registryPackages"]), 1)


if __name__ == "__main__":
    unittest.main()
