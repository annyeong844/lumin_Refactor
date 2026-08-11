#!/usr/bin/env python3
"""Focused REVIEW-003 bootstrap tests.

This file is executed as its own process. Fixtures use only temporary Cargo
homes and never inherit a real user's Cargo configuration or registry state.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import textwrap
import unittest
from unittest import mock


sys.dont_write_bytecode = True
SCRIPT = Path(__file__).with_name("source_provenance.py").resolve()
SPEC = importlib.util.spec_from_file_location("lumin_source_provenance", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
PROVENANCE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PROVENANCE
SPEC.loader.exec_module(PROVENANCE)


class Fixture:
    def __init__(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.base = Path(self.temporary.name).resolve()
        self.root = self.base / "repo"
        self.cargo_home = self.base / "cargo-home"
        self.target = self.base / "target"
        self.root.mkdir()
        self.cargo_home.mkdir()
        self.target.mkdir()
        (self.root / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
        self.root_manifest = self.root / "Cargo.toml"
        self.root_manifest.write_text(
            textwrap.dedent(
                """\
                [workspace]
                resolver = "3"
                members = ["app", "tools/xtask"]

                [workspace.dependencies]
                serde = { version = "=1.0.0", features = ["std", "derive"] }
                unused = "=2.0.0"
                """
            ),
            encoding="utf-8",
        )
        self.app = self.root / "app"
        self.xtask = self.root / "tools" / "xtask"
        self.app.mkdir()
        self.xtask.mkdir(parents=True)
        (self.app / "Cargo.toml").write_text(
            textwrap.dedent(
                """\
                [package]
                name = "app"
                version = "0.1.0"
                edition = "2024"

                [features]
                default = []
                extra = ["serde/derive"]

                [dependencies]
                serde.workspace = true
                """
            ),
            encoding="utf-8",
        )
        (self.xtask / "Cargo.toml").write_text(
            textwrap.dedent(
                """\
                [package]
                name = "lumin-xtask"
                version = "0.1.0"
                edition = "2024"

                [dependencies]
                app = { path = "../../app" }
                serde.workspace = true
                """
            ),
            encoding="utf-8",
        )
        self.registry_manifest = (
            self.cargo_home / "registry" / "src" / "index" / "serde-1.0.0" / "Cargo.toml"
        )
        self.registry_manifest.parent.mkdir(parents=True)
        self.registry_manifest.write_text(
            '[package]\nname = "serde"\nversion = "1.0.0"\n', encoding="utf-8"
        )

    def __enter__(self) -> "Fixture":
        return self

    def __exit__(self, *_: object) -> None:
        self.temporary.cleanup()

    @property
    def environment(self) -> dict[str, str]:
        return {
            "CARGO_HOME": str(self.cargo_home),
            "CARGO_TARGET_DIR": str(self.target),
        }

    def inspect(self) -> PROVENANCE.Repository:
        return PROVENANCE.inspect_repository(self.root, self.environment, self.root)

    def write_policy(self, policy: dict[str, object]) -> None:
        path = self.root / PROVENANCE.POLICY_PATH
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(policy), encoding="utf-8")

    @staticmethod
    def dependency(
        name: str,
        *,
        source: str | None,
        kind: str | None = None,
        path: str | None = None,
        features: list[str] | None = None,
    ) -> dict[str, object]:
        value: dict[str, object] = {
            "name": name,
            "source": source,
            "req": "*" if source is None else "=1.0.0",
            "kind": kind,
            "rename": None,
            "optional": False,
            "uses_default_features": True,
            "features": features or [],
            "target": None,
            "registry": None,
        }
        if path is not None:
            value["path"] = path
        return value

    @staticmethod
    def resolve_dependency(name: str, package_id: str, kind: str | None = None) -> dict[str, object]:
        return {
            "name": name,
            "pkg": package_id,
            "dep_kinds": [{"kind": kind, "target": None}],
        }

    def metadata(self) -> dict[str, object]:
        app_id = "path+app#app@0.1.0"
        xtask_id = "path+xtask#lumin-xtask@0.1.0"
        serde_id = f"{PROVENANCE.CRATES_IO_SOURCE}#serde@1.0.0"
        return {
            "workspace_root": str(self.root),
            "workspace_members": [app_id, xtask_id],
            "packages": [
                {
                    "id": app_id,
                    "name": "app",
                    "version": "0.1.0",
                    "source": None,
                    "manifest_path": str(self.app / "Cargo.toml"),
                    "dependencies": [
                        self.dependency(
                            "serde",
                            source=PROVENANCE.CRATES_IO_SOURCE,
                            features=["derive", "std"],
                        )
                    ],
                },
                {
                    "id": xtask_id,
                    "name": "lumin-xtask",
                    "version": "0.1.0",
                    "source": None,
                    "manifest_path": str(self.xtask / "Cargo.toml"),
                    "dependencies": [
                        self.dependency("app", source=None, path=str(self.app)),
                        self.dependency(
                            "serde",
                            source=PROVENANCE.CRATES_IO_SOURCE,
                            features=["derive", "std"],
                        ),
                    ],
                },
                {
                    "id": serde_id,
                    "name": "serde",
                    "version": "1.0.0",
                    "source": PROVENANCE.CRATES_IO_SOURCE,
                    "manifest_path": str(self.registry_manifest),
                    "dependencies": [],
                },
            ],
            "resolve": {
                "nodes": [
                    {
                        "id": app_id,
                        "deps": [self.resolve_dependency("serde", serde_id)],
                        "features": ["default", "extra"],
                    },
                    {
                        "id": xtask_id,
                        "deps": [
                            self.resolve_dependency("app", app_id),
                            self.resolve_dependency("serde", serde_id),
                        ],
                        "features": [],
                    },
                    {"id": serde_id, "deps": [], "features": ["derive", "std"]},
                ]
            },
        }


class CommandSurfaceTests(unittest.TestCase):
    def test_native_and_musl_commands_are_exactly_admitted(self) -> None:
        native = PROVENANCE.validate_command(
            ("cargo", "test", "--workspace", "--locked", "--", "--target", "fixture")
        )
        self.assertIsNone(native.explicit_target)
        musl = PROVENANCE.validate_command(
            (
                "cargo",
                "build",
                "--locked",
                "--target",
                "x86_64-unknown-linux-musl",
            )
        )
        self.assertEqual(musl.explicit_target, "x86_64-unknown-linux-musl")

    def test_clippy_maps_to_the_direct_driver_with_leading_token(self) -> None:
        plan = PROVENANCE.validate_command(
            ("cargo", "clippy", "--workspace", "--locked", "--", "-D", "warnings")
        )
        clippy = Path("/trusted/cargo-clippy")
        with mock.patch.object(PROVENANCE, "pinned_clippy", return_value=clippy):
            command = PROVENANCE.resolved_command(plan, Path("/trusted/cargo"), {}, Path("/repo"))
        self.assertEqual(
            command,
            (
                str(clippy),
                "clippy",
                "--workspace",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        )

    def test_mutating_relocating_and_ambiguous_commands_fail(self) -> None:
        rejected = (
            ("cargo", "test"),
            ("cargo", "test", "--locked", "--locked"),
            ("cargo", "test", "--", "--locked"),
            ("cargo", "update", "--locked"),
            ("cargo", "install", "--locked"),
            ("cargo", "vendor", "--locked"),
            ("cargo", "publish", "--locked"),
            ("cargo", "fmt", "--locked"),
            ("cargo", "+stable", "test", "--locked"),
            ("cargo", "test", "--locked", "--config", "source.x=y"),
            ("cargo", "test", "--locked", "--config=source.x=y"),
            ("cargo", "test", "--locked", "--manifest-path=x"),
            ("cargo", "test", "--locked", "--lockfile-path=x"),
            ("cargo", "test", "--locked", "-Cother"),
            ("cargo", "test", "--locked", "-Zunstable"),
            ("cargo", "test", "--locked", "--target=x86_64-unknown-linux-musl"),
            ("cargo", "test", "--locked", "--target", "aarch64-unknown-linux-gnu"),
        )
        for command in rejected:
            with self.subTest(command=command), self.assertRaises(PROVENANCE.ProvenanceError):
                PROVENANCE.validate_command(command)

    def test_windows_environment_matching_is_case_insensitive(self) -> None:
        for environment in (
            {"cargo_alias_audit": "test"},
            {"rustc": "payload"},
            {"cargo_source_crates_io_replace_with": "other"},
            {"cargo_registries_crates_io_index": "other"},
            {"cargo_build_target": "other"},
        ):
            with self.subTest(environment=environment), self.assertRaises(
                PROVENANCE.ProvenanceError
            ):
                PROVENANCE.reject_environment_overrides(environment, case_insensitive=True)


class DependencySurfaceTests(unittest.TestCase):
    def test_policy_is_small_direct_and_includes_the_development_tool(self) -> None:
        with Fixture() as fixture:
            policy = PROVENANCE.build_policy(fixture.inspect(), fixture.metadata())
        self.assertEqual(policy["schemaVersion"], 2)
        self.assertEqual([member["name"] for member in policy["members"]], ["app", "lumin-xtask"])
        xtask = policy["members"][1]
        self.assertEqual(xtask["class"], "development-tool")
        self.assertEqual(len(xtask["dependencies"]), 2)
        self.assertIn("unused", [entry["alias"] for entry in policy["workspaceDependencies"]])
        self.assertNotIn("transitivePackages", policy)

    def test_metadata_traversal_and_feature_set_order_do_not_change_policy(self) -> None:
        with Fixture() as fixture:
            repository = fixture.inspect()
            metadata = fixture.metadata()
            expected = PROVENANCE.build_policy(repository, metadata)
            reordered = copy.deepcopy(metadata)
            reordered["packages"].reverse()
            reordered["workspace_members"].reverse()
            reordered["resolve"]["nodes"].reverse()
            for package in reordered["packages"]:
                package["dependencies"].reverse()
                for dependency in package["dependencies"]:
                    dependency["features"].reverse()
            for node in reordered["resolve"]["nodes"]:
                node["deps"].reverse()
            actual = PROVENANCE.build_policy(repository, reordered)
        self.assertEqual(actual, expected)

    def test_authored_requirement_remains_exact(self) -> None:
        with Fixture() as fixture:
            metadata = fixture.metadata()
            before = PROVENANCE.build_policy(fixture.inspect(), metadata)
            source = fixture.root_manifest.read_text(encoding="utf-8")
            fixture.root_manifest.write_text(
                source.replace('"=1.0.0"', '"= 1.0.0"'), encoding="utf-8"
            )
            after = PROVENANCE.build_policy(fixture.inspect(), metadata)
        self.assertIsNotNone(PROVENANCE._first_difference(before, after))
        self.assertEqual(after["workspaceDependencies"][0]["requirement"], "= 1.0.0")

    def test_requested_feature_order_is_set_canonical(self) -> None:
        with Fixture() as fixture:
            metadata = fixture.metadata()
            before = PROVENANCE.build_policy(fixture.inspect(), metadata)
            source = fixture.root_manifest.read_text(encoding="utf-8")
            fixture.root_manifest.write_text(
                source.replace('["std", "derive"]', '["derive", "std"]'),
                encoding="utf-8",
            )
            after = PROVENANCE.build_policy(fixture.inspect(), metadata)
        self.assertEqual(after, before)

    def test_absent_and_explicit_empty_feature_requests_remain_distinct(self) -> None:
        with Fixture() as fixture:
            metadata = fixture.metadata()
            absent = PROVENANCE.build_policy(fixture.inspect(), metadata)
            source = fixture.root_manifest.read_text(encoding="utf-8")
            fixture.root_manifest.write_text(
                source.replace(
                    'unused = "=2.0.0"',
                    'unused = { version = "=2.0.0", features = [] }',
                ),
                encoding="utf-8",
            )
            explicit_empty = PROVENANCE.build_policy(fixture.inspect(), metadata)
        self.assertIsNone(absent["workspaceDependencies"][1]["features"])
        self.assertEqual(explicit_empty["workspaceDependencies"][1]["features"], [])
        self.assertIsNotNone(PROVENANCE._first_difference(absent, explicit_empty))
        with Fixture() as fixture:
            metadata = fixture.metadata()
            absent = PROVENANCE.build_policy(fixture.inspect(), metadata)
            manifest = fixture.app / "Cargo.toml"
            manifest.write_text(
                manifest.read_text(encoding="utf-8").replace(
                    "serde.workspace = true",
                    "serde = { workspace = true, features = [] }",
                ),
                encoding="utf-8",
            )
            explicit_empty = PROVENANCE.build_policy(fixture.inspect(), metadata)
        self.assertIsNone(absent["members"][0]["dependencies"][0]["features"])
        self.assertEqual(explicit_empty["members"][0]["dependencies"][0]["features"], [])
        self.assertIsNotNone(PROVENANCE._first_difference(absent, explicit_empty))

    def test_member_and_development_tool_declaration_drift_fail(self) -> None:
        with Fixture() as fixture:
            metadata = fixture.metadata()
            for manifest in (fixture.app / "Cargo.toml", fixture.xtask / "Cargo.toml"):
                original = manifest.read_text(encoding="utf-8")
                manifest.write_text(
                    original.replace("serde.workspace = true", "serde = { workspace = true, optional = true }"),
                    encoding="utf-8",
                )
                with self.subTest(manifest=manifest), self.assertRaises(
                    PROVENANCE.ProvenanceError
                ):
                    PROVENANCE.build_policy(fixture.inspect(), metadata)
                manifest.write_text(original, encoding="utf-8")

    def test_policy_comparison_rejects_feature_and_origin_drift(self) -> None:
        replacements = (
            ('default = []', 'default = ["serde/std"]'),
            (
                "serde.workspace = true",
                'serde = { version = "=1.0.0", features = ["derive", "std"] }',
            ),
        )
        for old, new in replacements:
            with Fixture() as fixture:
                metadata = fixture.metadata()
                repository = fixture.inspect()
                fixture.write_policy(PROVENANCE.build_policy(repository, metadata))
                manifest = fixture.app / "Cargo.toml"
                manifest.write_text(
                    manifest.read_text(encoding="utf-8").replace(old, new),
                    encoding="utf-8",
                )
                changed = fixture.inspect()
                with self.subTest(replacement=new), mock.patch.object(
                    PROVENANCE,
                    "run_metadata",
                    side_effect=[copy.deepcopy(metadata), copy.deepcopy(metadata)],
                ), self.assertRaises(PROVENANCE.ProvenanceError):
                    PROVENANCE.dependency_preflight(
                        changed,
                        Path("cargo"),
                        "x86_64-unknown-linux-gnu",
                        None,
                        fixture.environment,
                    )

    def test_duplicate_and_unresolved_direct_bindings_fail(self) -> None:
        for mutation in ("duplicate", "remove"):
            with Fixture() as fixture:
                metadata = fixture.metadata()
                app_node = metadata["resolve"]["nodes"][0]
                if mutation == "duplicate":
                    app_node["deps"].append(copy.deepcopy(app_node["deps"][0]))
                else:
                    app_node["deps"].clear()
                with self.subTest(mutation=mutation), self.assertRaises(
                    PROVENANCE.ProvenanceError
                ):
                    PROVENANCE.build_policy(fixture.inspect(), metadata)

    def test_filtered_lane_must_match_the_selected_direct_bindings(self) -> None:
        with Fixture() as fixture:
            repository = fixture.inspect()
            metadata = fixture.metadata()
            policy = PROVENANCE.build_policy(repository, metadata)
            PROVENANCE.validate_filtered_lane(
                policy, metadata, repository, "x86_64-unknown-linux-gnu"
            )
            changed = copy.deepcopy(policy)
            changed["members"][0]["dependencies"][0]["target"] = "cfg(windows)"
            with self.assertRaises(PROVENANCE.ProvenanceError):
                PROVENANCE.validate_filtered_lane(
                    changed, metadata, repository, "x86_64-unknown-linux-gnu"
                )

    def test_registry_manifest_must_be_loaded_from_the_active_cargo_home(self) -> None:
        with Fixture() as fixture:
            metadata = fixture.metadata()
            metadata["packages"][2]["manifest_path"] = str(fixture.root / "Cargo.toml")
            with self.assertRaises(PROVENANCE.ProvenanceError):
                PROVENANCE.build_policy(fixture.inspect(), metadata)

    def test_root_package_external_member_and_source_override_fail(self) -> None:
        cases = (
            ("\n[package]\nname = \"root\"\nversion = \"0.1.0\"\n", None),
            (None, ('members = ["app", "tools/xtask"]', 'members = ["../outside"]')),
            (None, ('unused = "=2.0.0"', 'unused = { git = "https://example.invalid/x" }')),
        )
        for suffix, replacement in cases:
            with Fixture() as fixture:
                source = fixture.root_manifest.read_text(encoding="utf-8")
                if suffix:
                    source += suffix
                if replacement:
                    source = source.replace(*replacement)
                fixture.root_manifest.write_text(source, encoding="utf-8")
                with self.subTest(source=source), self.assertRaises(PROVENANCE.ProvenanceError):
                    fixture.inspect()

    def test_cargo_config_patch_and_alternate_registry_fail_before_metadata(self) -> None:
        with Fixture() as fixture:
            config = fixture.root / ".cargo" / "config.toml"
            config.parent.mkdir()
            config.write_text("[source.crates-io]\nreplace-with='other'\n", encoding="utf-8")
            with self.assertRaises(PROVENANCE.ProvenanceError):
                fixture.inspect()
        with Fixture() as fixture:
            fixture.root_manifest.write_text(
                fixture.root_manifest.read_text(encoding="utf-8")
                + "\n[patch.crates-io]\nserde = { path = 'app' }\n",
                encoding="utf-8",
            )
            with self.assertRaises(PROVENANCE.ProvenanceError):
                fixture.inspect()
        with Fixture() as fixture:
            manifest = fixture.app / "Cargo.toml"
            manifest.write_text(
                manifest.read_text(encoding="utf-8").replace(
                    "serde.workspace = true",
                    'serde = { version = "=1.0.0", registry = "other" }',
                ),
                encoding="utf-8",
            )
            with self.assertRaises(PROVENANCE.ProvenanceError):
                PROVENANCE.build_policy(fixture.inspect(), fixture.metadata())

    def test_duplicate_and_unknown_policy_keys_fail(self) -> None:
        with Fixture() as fixture:
            path = fixture.root / PROVENANCE.POLICY_PATH
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('{"schemaVersion":2,"schemaVersion":3}', encoding="utf-8")
            with self.assertRaises(PROVENANCE.ProvenanceError):
                PROVENANCE.load_policy(fixture.root)
            with self.assertRaises(PROVENANCE.ProvenanceError):
                PROVENANCE._validate_policy_shape(
                    {
                        "schemaVersion": 2,
                        "resolver": "3",
                        "members": [],
                        "workspaceDependencies": [],
                        "unexpected": True,
                    }
                )


if __name__ == "__main__":
    unittest.main(verbosity=2)
